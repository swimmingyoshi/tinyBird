"""Train/evaluate a local PPO or DQN agent using Workshop observations and a JSON task."""
import argparse
from datetime import datetime, timezone
import hashlib
import importlib.metadata
import json
from pathlib import Path
import time

import numpy as np
import torch
from stable_baselines3 import PPO, DQN
from stable_baselines3.common.callbacks import CheckpointCallback, BaseCallback
from stable_baselines3.common.env_checker import check_env
from stable_baselines3.common.monitor import Monitor

from tinybird_env import TinyBirdEnv, read_json
from hybrid_policy import HybridExtractor


class CurriculumEvalCallback(BaseCallback):
    """Compare every checkpoint on every configured start, never a lucky subset."""
    def __init__(self, env, directory, seed, frequency=512):
        super().__init__()
        self.env, self.directory, self.seed, self.frequency = env, Path(directory), seed, frequency
        self.best_rank = None

    def _on_step(self):
        if self.n_calls % self.frequency == 0:
            result = evaluate_suite(self.env, self.model, self.seed)
            rank = selection_rank(result)
            self.directory.mkdir(parents=True, exist_ok=True)
            (self.directory / f'{self.num_timesteps}.json').write_text(json.dumps(result, indent=2))
            if self.best_rank is None or rank > self.best_rank:
                self.best_rank = rank
                self.model.save(self.directory / 'best_model.zip')
        return True


def selection_rank(result):
    # Completion first, then productive score/progress/time tradeoffs. A detour
    # earning points may legitimately beat running farther without scoring.
    progress = [e['metric_gains'].get('world_x', 0) for e in result['episodes'] if e['outcome'] != 'success']
    return (result.get('requirements_met', True), result['success_rate'], result['mean_reward'], min(progress, default=0))


def evaluate_suite(env, model, seed):
    variants = range(len(env.unwrapped.task['reset_variants'])) if env.unwrapped.task.get('reset_variants') else [None]
    reports = [evaluate(env, model, 1, seed, start_variant=v) for v in variants]
    episodes = [e for report in reports for e in report['episodes']]
    checks = []
    for rule in env.unwrapped.task.get('evaluation_requirements', []):
        gains = [e['metric_gains'][rule['metric']] for e in episodes if e['reset_variant'] == rule['variant']]
        checks.append(dict(rule, observed_gains=gains, passed=bool(gains) and min(gains) >= rule['min_gain']))
    return {'mean_reward': float(np.mean([e['reward'] for e in episodes])),
            'success_rate': sum(e['outcome'] == 'success' for e in episodes) / len(episodes),
            'episodes': episodes, 'requirements':checks, 'requirements_met':all(c['passed'] for c in checks)}

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest() if path else None


def initialize_actor(model, run, signature):
    """Explicit transfer to an expanded action space; never treat it as resume."""
    manifest, _, source = load_run(run)
    old = manifest['signature']
    for key in ('rom', 'bios', 'state', 'runtime'):
        if old[key] != signature[key]:
            raise ValueError(f'Policy initialization requires matching {key}')
    before, after = old['observations'], signature['observations']
    if before != after:
        # Extra reward/diagnostic fields are safe when the policy inputs below
        # and every original memory definition remain unchanged.
        if not isinstance(before, dict) or not isinstance(after, dict) or (
            {k:v for k,v in before.items() if k != 'fields'} != {k:v for k,v in after.items() if k != 'fields'}
            or any(field not in after.get('fields', []) for field in before.get('fields', []))):
            raise ValueError('Policy initialization requires unchanged existing observations')
    a, b = old['task'], signature['task']
    for key in ('features', 'history', 'visual', 'release_frames', 'release_buttons'):
        if a.get(key) != b.get(key):
            raise ValueError(f'Policy initialization requires matching {key}')
    n = len(a['actions'])
    old_durations = a.get('action_frames', [a['frame_skip']] * n)
    new_durations = b.get('action_frames', [b['frame_skip']] * len(b['actions']))
    if b['actions'][:n] != a['actions'] or new_durations[:n] != old_durations:
        raise ValueError('Existing action meanings and durations must be preserved')
    if not isinstance(source, PPO) or not isinstance(model, PPO):
        raise ValueError('Policy initialization supports PPO only')
    weights = model.policy.state_dict()
    for key, value in source.policy.state_dict().items():
        if key in ('action_net.weight', 'action_net.bias'):
            weights[key][:n] = value.to(model.device)
        elif key in weights and weights[key].shape == value.shape:
            weights[key] = value.to(model.device)
        else:
            raise ValueError(f'Incompatible policy parameter: {key}')
    model.policy.load_state_dict(weights)

def load_run(run):
    """Load trusted local weights and verify the exact emulator inputs."""
    run = Path(run)
    manifest = read_json(run / 'manifest.json')
    inputs = manifest['inputs']
    for key in ['rom', 'bios', 'state', 'runtime']:
        if digest(inputs['executable' if key == 'runtime' else key]) != manifest['signature'][key]:
            raise ValueError(f'{key} differs from the training run')
    kwargs = dict(inputs, observations=manifest['signature']['observations'], task=manifest['signature']['task'])
    model = {'ppo': PPO, 'dqn': DQN}[manifest.get('algorithm', 'ppo')].load(run / 'model.zip', device='cpu')
    return manifest, kwargs, model

def evaluate(env, model, count, seed, record=None, deterministic=True, start_variant=None, stratified=False):
    episodes = []
    for episode in range(count):
        variant = start_variant
        if variant is None and stratified and env.unwrapped.task.get('reset_variants'):
            variant = episode % len(env.unwrapped.task['reset_variants'])
        observation, info = env.reset(seed=seed + episode, options={'variant':variant} if variant is not None else None)
        reset_choice = info['reset_choice']
        reset_variant = info.get('reset_variant')
        initial_metrics = dict(info.get('metrics', {}))
        peak_metrics = dict(initial_metrics)
        reward_totals = {}
        env.action_space.seed(seed + episode)
        if model is not None and not deterministic:
            model.set_random_seed(seed + episode)
        total, steps, frames = 0, 0, []
        durations = []
        while True:
            action = env.action_space.sample() if model is None else int(model.predict(observation, deterministic=deterministic)[0])
            observation, reward, terminated, truncated, info = env.step(action)
            total += reward; steps += 1
            for key, value in info.get('metrics', {}).items():
                peak_metrics[key] = max(peak_metrics.get(key, value), value)
            for key, value in info.get('reward_terms', {}).items():
                reward_totals[key] = reward_totals.get(key, 0) + value
            if record and episode == 0 and len(frames) < 1000:
                from PIL import Image
                frames.append(Image.fromarray(env.render()).resize((480, 320), Image.Resampling.NEAREST))
                durations.append(max(20, round(1000 * info['action_frames'] / 60)))
            if terminated or truncated:
                break
        episodes.append({'reward': total, 'steps': steps, 'outcome': info.get('outcome') or info.get('truncation_reason') or 'time_limit', 'seed': seed + episode, 'reset_choice': reset_choice,
                         'elapsed_seconds': info.get('elapsed_seconds'), 'elapsed_frames': info.get('elapsed_frames'),
                         'reset_variant': reset_variant, 'metrics': info.get('metrics', {}), 'peak_metrics': peak_metrics,
                         'metric_gains': {k:v-initial_metrics[k] for k,v in info.get('metrics', {}).items()}, 'reward_terms': reward_totals})
        if frames:
            frames[0].save(record, save_all=True, append_images=frames[1:], duration=durations, loop=0)
    by_start = {}
    for variant in {e['reset_variant'] for e in episodes}:
        group = [e for e in episodes if e['reset_variant'] == variant]
        by_start[str(variant)] = {'episodes':len(group),
            'success_rate':sum(e['outcome'] == 'success' for e in group) / len(group),
            'mean_reward':float(np.mean([e['reward'] for e in group]))}
    return {'mean_reward': float(np.mean([e['reward'] for e in episodes])),
            'success_rate': sum(e['outcome'] == 'success' for e in episodes) / count, 'by_start':by_start, 'episodes': episodes}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('rom'); parser.add_argument('observations'); parser.add_argument('task')
    parser.add_argument('--executable', default='tinybird-headless')
    parser.add_argument('--bios'); parser.add_argument('--state')
    parser.add_argument('--steps', type=int, default=10000)
    parser.add_argument('--seed', type=int, default=0)
    parser.add_argument('--algorithm', choices=['ppo', 'dqn'], default='ppo')
    parser.add_argument('--device', choices=['auto', 'cpu', 'cuda'], default='auto')
    parser.add_argument('--demonstrations', type=Path, help='Warm-start PPO from verified action sequences before reinforcement learning')
    parser.add_argument('--demonstration-epochs', type=int, default=200)
    parser.add_argument('--rehearsal-epochs', type=int, default=0, help='Rehearse demonstrations after PPO, then compare against unrehearsed checkpoints')
    parser.add_argument('--eval-episodes', type=int, default=20)
    parser.add_argument('--output', type=Path, default=None)
    source = parser.add_mutually_exclusive_group()
    source.add_argument('--evaluate', type=Path, help='Evaluate model.zip in a previous run directory')
    source.add_argument('--resume', type=Path, help='Continue model.zip from a previous run into a new directory')
    source.add_argument('--initialize-from', type=Path, help='Transfer a compatible PPO policy into an expanded action space with a fresh optimizer')
    parser.add_argument('--record', action='store_true', help='Write the first evaluation episode as a GIF (up to 1000 frames)')
    parser.add_argument('--stochastic', action='store_true', help='Evaluate seeded policy samples instead of the most likely action')
    args = parser.parse_args()
    if args.steps < 1 or args.eval_episodes < 1 or args.seed < 0:
        parser.error('steps/eval-episodes must be positive and seed nonnegative')
    if not 0 <= args.rehearsal_epochs <= 1000 or (args.rehearsal_epochs and not args.demonstrations):
        parser.error('rehearsal-epochs must be 0–1000 and requires demonstrations')
    if not 1 <= args.demonstration_epochs <= 2000:
        parser.error('demonstration-epochs must be 1–2000')
    device = ('cuda' if torch.cuda.is_available() else 'cpu') if args.device == 'auto' else args.device
    if device == 'cuda' and not torch.cuda.is_available():
        parser.error('CUDA is unavailable in this Python environment; use CPU or install a compatible CUDA build of PyTorch')
    import shutil
    executable = shutil.which(args.executable) or str(Path(args.executable).resolve())
    signature = {name: digest(value) for name, value in [('rom', args.rom), ('bios', args.bios), ('state', args.state), ('runtime', executable)]}
    signature.update(observations=read_json(args.observations), task=read_json(args.task))
    previous_run = args.evaluate or args.resume
    if previous_run:
        saved = read_json(previous_run / 'manifest.json')
        args.algorithm = saved.get('algorithm', 'ppo')
        if saved['signature'] != signature:
            raise ValueError('The ROM, BIOS, starting state, runtime, observations or task differ from this model’s training run')
    if args.demonstrations and (args.algorithm != 'ppo' or args.evaluate):
        parser.error('Demonstration warm-start is supported for PPO training only')
    output = args.output or Path('target/training') / datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
    output.mkdir(parents=True, exist_ok=False)
    manifest = {'signature': signature, 'seed': args.seed, 'requested_steps': args.steps,
                'versions': {name: importlib.metadata.version(name) for name in ['gymnasium', 'stable-baselines3', 'torch', 'numpy']},
                'evaluation_source': str(args.evaluate) if args.evaluate else None,
                'resume_source': str(args.resume) if args.resume else None}
    manifest['policy_mode'] = 'stochastic' if args.stochastic else 'deterministic'
    manifest['algorithm'] = args.algorithm
    manifest['device'] = device
    manifest['training_code'] = {name: digest(Path(__file__).with_name(name)) for name in ['train.py', 'tinybird_env.py', 'tinybird.py', 'hybrid_policy.py', 'demonstrations.py']}
    visual = bool(signature['task'].get('visual'))
    manifest['network'] = 'hybrid_cnn_ram' if visual else 'ram_mlp'
    manifest['separate_actor_critic'] = saved.get('separate_actor_critic', False) if previous_run else visual and args.algorithm == 'ppo'
    manifest['demonstrations'] = read_json(args.demonstrations) if args.demonstrations else None
    manifest['rehearsal_epochs'] = args.rehearsal_epochs
    manifest['demonstration_epochs'] = args.demonstration_epochs
    manifest['initialization_source'] = str(args.initialize_from) if args.initialize_from else None
    manifest['initialization_weights_sha256'] = digest(args.initialize_from / 'model.zip') if args.initialize_from else None
    manifest['inputs'] = {key: str(Path(value).resolve()) if value else None for key, value in
                          [('rom', args.rom), ('observations', args.observations), ('task', args.task),
                           ('bios', args.bios), ('state', args.state), ('executable', executable)]}
    (output / 'manifest.json').write_text(json.dumps(manifest, indent=2))
    print('Run configuration:', json.dumps({'task':str(args.task), 'source':str(args.resume or args.initialize_from or args.evaluate) if (args.resume or args.initialize_from or args.evaluate) else None,
        'separate_actor_critic':manifest['separate_actor_critic'], 'evaluation_requirements':signature['task'].get('evaluation_requirements', [])}), flush=True)
    torch.set_num_threads(1)
    kwargs = dict(rom=args.rom, observations=args.observations, task=args.task, executable=executable, bios=args.bios, state=args.state)
    env = evaluation = validation = None
    algorithm = {'ppo': PPO, 'dqn': DQN}[args.algorithm]
    started = time.monotonic()
    trained_timesteps = None
    policy = 'MultiInputPolicy' if visual else 'MlpPolicy'
    policy_kwargs = {'net_arch': [128, 64], 'features_extractor_class': HybridExtractor} if visual else {'net_arch': [64, 64]}
    if visual and args.algorithm == 'ppo':
        policy_kwargs['share_features_extractor'] = False
    try:
        env = Monitor(TinyBirdEnv(**kwargs), str(output / 'episodes'))
        check_env(env.unwrapped, warn=True)
        evaluation = TinyBirdEnv(**kwargs, render_mode='rgb_array' if args.record else None)
        baseline = evaluate(evaluation, None, args.eval_episodes, args.seed + 10000, stratified=True)
        print('Random baseline:', json.dumps({k:v for k,v in baseline.items() if k != 'episodes'}), flush=True)
        if args.evaluate:
            model = algorithm.load(args.evaluate / 'model.zip', device=device)
        else:
            if args.resume:
                model = algorithm.load(args.resume / 'model.zip', env=env, device=device)
                model.set_random_seed(args.seed)
            elif args.algorithm == 'ppo':
                model = PPO(policy, env, device=device, seed=args.seed, n_steps=256, batch_size=64,
                            n_epochs=4 if visual else 10, ent_coef=.01 if visual else 0,
                            learning_rate=1e-4 if visual else 3e-4, target_kl=.03 if visual else None,
                            gamma=.99, verbose=1, policy_kwargs=policy_kwargs)
            else:
                model = DQN(policy, env, device=device, seed=args.seed, learning_rate=3e-4 if visual else 1e-3,
                            buffer_size=4096 if visual else 50000, learning_starts=256, batch_size=32 if visual else 64, train_freq=4,
                            gradient_steps=1, target_update_interval=500, exploration_fraction=.65,
                            exploration_final_eps=.05, gamma=.98, verbose=1, policy_kwargs=policy_kwargs)
            (output / 'network.txt').write_text(str(model.policy))
            if args.initialize_from:
                initialize_actor(model, args.initialize_from, signature)
            if args.initialize_from or args.resume:
                model.save(output / 'source_model.zip')
            if args.demonstrations:
                from demonstrations import warm_start
                demonstration_report = warm_start(model, env, manifest['demonstrations'], epochs=args.demonstration_epochs)
                (output / 'demonstrations.json').write_text(json.dumps(demonstration_report, indent=2))
                print('Demonstration warm-start:', json.dumps(demonstration_report), flush=True)
            validation = Monitor(TinyBirdEnv(**kwargs))
            model.save(output / 'initial_model.zip')
            initial_validation = evaluate_suite(validation, model, args.seed + 20000)
            (output / 'initial_validation.json').write_text(json.dumps(initial_validation, indent=2))
            print('Initial validation:', json.dumps({'requirements_met':initial_validation['requirements_met'],
                'starts':[{'variant':e['reset_variant'], 'outcome':e['outcome'], 'gains':e['metric_gains']} for e in initial_validation['episodes']]}), flush=True)
            selection = CurriculumEvalCallback(validation, output / 'best', args.seed + 20000)
            model.learn(total_timesteps=args.steps, reset_num_timesteps=not bool(args.resume), callback=[
                CheckpointCallback(save_freq=2048, save_path=str(output / 'checkpoints')), selection])
            model.save(output / 'final_model.zip')
            trained_timesteps = model.num_timesteps
            final_validation = evaluate_suite(validation, model, args.seed + 20000)
            best_path = output / 'best' / 'best_model.zip'
            selection_report = {'final': final_validation, 'selected': 'final_model.zip', 'ranking': ['requirements_met', 'completion_rate', 'balanced_reward', 'worst_start_net_progress']}
            initial = algorithm.load(output / 'initial_model.zip', device=device)
            selection_report['initial'] = initial_validation
            selected_reward = selection_rank(final_validation)
            if selection_rank(initial_validation) > selected_reward:
                model = initial
                selected_reward = selection_rank(initial_validation)
                selection_report['selected'] = 'initial_model.zip'
            if best_path.exists():
                best = algorithm.load(best_path, device=device)
                best_validation = evaluate_suite(validation, best, args.seed + 20000)
                selection_report['best_checkpoint'] = best_validation
                if selection_rank(best_validation) > selected_reward:
                    model = best
                    selected_reward = selection_rank(best_validation)
                    selection_report['selected'] = 'best/best_model.zip'
            if args.rehearsal_epochs:
                rehearsed = algorithm.load(output / 'final_model.zip', device=device)
                rehearsal_report = warm_start(rehearsed, env, manifest['demonstrations'], epochs=args.rehearsal_epochs)
                rehearsed.save(output / 'rehearsed_model.zip')
                rehearsal_validation = evaluate_suite(validation, rehearsed, args.seed + 20000)
                selection_report['rehearsal'] = rehearsal_validation
                selection_report['rehearsal_training'] = rehearsal_report
                if selection_rank(rehearsal_validation) > selected_reward:
                    model = rehearsed
                    selected_reward = selection_rank(rehearsal_validation)
                    selection_report['selected'] = 'rehearsed_model.zip'
            if (output / 'source_model.zip').exists():
                source_model = algorithm.load(output / 'source_model.zip', device=device)
                source_validation = evaluate_suite(validation, source_model, args.seed + 20000)
                selection_report['source'] = source_validation
                if selection_rank(source_validation) > selected_reward:
                    model = source_model
                    selected_reward = selection_rank(source_validation)
                    selection_report['selected'] = 'source_model.zip'
            selection_report['requirements_met'] = selected_reward[0]
            (output / 'selection.json').write_text(json.dumps(selection_report, indent=2))
            if not selected_reward[0]:
                raise RuntimeError('No candidate met the evaluation requirements; inspect selection.json. No model.zip was promoted.')
        model.save(output / 'model.zip')
        trained = evaluate(evaluation, model, args.eval_episodes, args.seed + 10000, output / 'evaluation.gif' if args.record else None, deterministic=not args.stochastic, stratified=True)
        result = {'random': baseline, 'policy': trained, 'seconds': time.monotonic() - started,
                  'trained_timesteps': trained_timesteps,
                  'model_timesteps': model.num_timesteps, 'policy_mode': manifest['policy_mode'], 'output': str(output)}
        (output / 'evaluation.json').write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2), flush=True)
    finally:
        if env: env.close()
        if evaluation: evaluation.close()
        if validation: validation.close()

if __name__ == '__main__':
    main()
