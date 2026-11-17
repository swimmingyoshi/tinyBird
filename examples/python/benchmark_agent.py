"""Compare a local policy against random/constant actions with delayed starts."""
import argparse
import json
from pathlib import Path

import torch

from tinybird_env import TinyBirdEnv, read_json
from train import evaluate, load_run


class ConstantPolicy:
    def __init__(self, action):
        self.action = action

    def predict(self, observation, deterministic=True):
        return self.action, None

    def set_random_seed(self, seed):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    parser.add_argument('--delays', type=int, nargs='+', default=[0, 4, 8, 16, 24, 32, 48, 64])
    parser.add_argument('--random-episodes', type=int, default=5)
    parser.add_argument('--start-variant', type=int, default=0, help='Compare every policy from the same starting checkpoint')
    args = parser.parse_args()
    if args.random_episodes < 1 or any(d < 0 for d in args.delays):
        parser.error('random-episodes must be positive and delays nonnegative')
    torch.set_num_threads(1)
    manifest, kwargs, model = load_run(args.run)
    variants = kwargs['task'].get('reset_variants', [[]])
    if not 0 <= args.start_variant < len(variants):
        parser.error('start-variant is outside the configured checkpoints')
    if kwargs['task'].get('reset_choices', [[]]) != [[]]:
        parser.error('Delayed-start benchmark requires a neutral reset_choices setting')
    results = []
    for delay in args.delays:
        task = read_json(kwargs['task'])
        if task.get('reset_variants'):
            task['reset_variants'] = [variants[args.start_variant]]
        task['reset_frames'] = task.get('reset_frames', 1) + delay
        env = TinyBirdEnv(**dict(kwargs, task=task))
        try:
            seed = manifest['seed'] + 30000
            result = {'delay_frames': delay, 'start_variant':args.start_variant, 'policy': evaluate(env, model, 1, seed),
                      'random': evaluate(env, None, args.random_episodes, seed),
                      'constant_actions': [evaluate(env, ConstantPolicy(a), 1, seed) for a in range(env.action_space.n)]}
            results.append(result)
            print(f"Delay {delay}: policy {result['policy']['success_rate']:.0%}, random {result['random']['success_rate']:.0%}", flush=True)
        finally:
            env.close()
    (args.run / 'delayed-starts.json').write_text(json.dumps(results, indent=2))


if __name__ == '__main__':
    main()
