"""Local Gymnasium adapter. Tasks are declarative JSON, never executable code."""
from collections import deque
import json
import math
from pathlib import Path

import gymnasium as gym
import numpy as np

from tinybird import TinyBird

BUTTONS = {'A', 'B', 'SELECT', 'START', 'RIGHT', 'LEFT', 'UP', 'DOWN', 'R', 'L'}
OPS = {'eq': lambda a, b: a == b, 'lt': lambda a, b: a < b, 'le': lambda a, b: a <= b,
       'gt': lambda a, b: a > b, 'ge': lambda a, b: a >= b}

def read_json(value):
    return json.loads(Path(value).read_text()) if isinstance(value, (str, Path)) else json.loads(json.dumps(value))

def validate_task(task, observations):
    allowed = {'schema_version', 'frame_skip', 'release_frames', 'release_buttons', 'visual', 'reset_variants', 'max_steps', 'history', 'reset_frames', 'actions', 'reset_choices', 'features', 'rewards', 'step_reward', 'terminal', 'reset_sequence', 'metrics', 'stall', 'action_frames', 'frame_reward', 'max_frames', 'evaluation_requirements'}
    if not isinstance(task, dict) or type(task.get('schema_version')) is not int or task['schema_version'] != 1:
        raise ValueError('Unsupported training task version')
    if set(task) - allowed:
        raise ValueError(f'Unknown training settings: {sorted(set(task) - allowed)}')
    names = {field['name'] for field in observations['fields']}
    visual = task.get('visual')
    if visual is not None and (not isinstance(visual, dict) or set(visual) != {'width', 'height'} or any(type(visual[k]) is not int or not 36 <= visual[k] <= 160 for k in visual)):
        raise ValueError('visual requires integer width and height between 36 and 160')
    release = task.get('release_buttons')
    if release is not None and (not isinstance(release, list) or not release or any(not isinstance(b, str) or b not in BUTTONS for b in release)):
        raise ValueError('release_buttons must contain valid buttons')
    for key, low, high, default in [('frame_skip', 1, 600, 4), ('release_frames', 0, 10, 0), ('max_steps', 1, 1_000_000, 1000), ('history', 1, 32, 4), ('reset_frames', 1, 600, 1)]:
        value = task.setdefault(key, default)
        if type(value) is not int or not low <= value <= high:
            raise ValueError(f'{key} must be an integer between {low} and {high}')
    for key in ['actions', 'reset_choices']:
        choices = task.setdefault(key, [[]]) if key == 'reset_choices' else task.get(key)
        if not isinstance(choices, list) or not 1 <= len(choices) <= 64:
            raise ValueError(f'{key} must contain 1–64 button combinations')
        for buttons in choices:
            if not isinstance(buttons, list) or not all(isinstance(b, str) and b in BUTTONS for b in buttons) or len(set(buttons)) != len(buttons):
                raise ValueError(f'Invalid {key} button combination')
    if not isinstance(task.get('features'), list) or not task['features']:
        raise ValueError('Choose at least one observation feature')
    def number(value):
        return type(value) in (int, float) and math.isfinite(value)
    for field in task['features']:
        if field.get('field') not in names or not number(field.get('min')) or not number(field.get('max')) or field['max'] <= field['min']:
            raise ValueError('Features need a known field and finite min < max')
    for reward in task.get('rewards', []):
        if reward.get('field') not in names or reward.get('mode') not in ('delta', 'progress') or not number(reward.get('scale')):
            raise ValueError('Rewards need a known field, delta/progress mode and finite scale')
    if not number(task.setdefault('step_reward', 0)):
        raise ValueError('step_reward must be finite')
    if not number(task.get('frame_reward', 0)):
        raise ValueError('frame_reward must be finite')
    durations = task.get('action_frames')
    if durations is not None and (not isinstance(durations, list) or len(durations) != len(task['actions']) or any(type(n) is not int or not 1 <= n <= 600 for n in durations)):
        raise ValueError('action_frames needs one duration between 1 and 600 per action')
    if 'max_frames' in task and (type(task['max_frames']) is not int or task['max_frames'] < 1):
        raise ValueError('max_frames must be a positive integer')
    for reward in task.get('rewards', []):
        if 'cap' in reward and (reward['mode'] != 'progress' or not number(reward['cap']) or reward['cap'] < 0):
            raise ValueError('Reward caps require progress mode and a nonnegative finite cap')
    metrics = task.setdefault('metrics', [])
    if not isinstance(metrics, list):
        raise ValueError('metrics must be a list')
    metric_names = set()
    for metric in metrics:
        if not isinstance(metric, dict) or not isinstance(metric.get('name'), str) or not metric['name'] or metric['name'] in metric_names or metric.get('field') not in names or not number(metric.get('scale', 1)):
            raise ValueError('Metrics need unique names, known fields and finite scales')
        metric_names.add(metric['name'])
    stall = task.get('stall')
    if stall is not None and (not isinstance(stall, dict) or stall.get('field') not in names or type(stall.get('steps')) is not int or not 1 <= stall['steps'] <= 1_000_000 or not number(stall.get('min_delta')) or stall['min_delta'] <= 0 or not number(stall.get('reward', 0))):
        raise ValueError('stall needs a known field, positive steps and positive min_delta')
    if stall is not None:
        if 'frames' in stall and (type(stall['frames']) is not int or stall['frames'] < 1):
            raise ValueError('stall frames must be a positive integer')
        productive = stall.get('productive_fields', [])
        if not isinstance(productive, list) or any(not isinstance(f, str) or f not in names for f in productive):
            raise ValueError('stall productive_fields must name observation fields')
    for rule in task.get('terminal', []):
        if rule.get('field') not in names or rule.get('op') not in OPS or rule.get('outcome') not in ('success', 'failure') or not number(rule.get('value')) or not number(rule.get('reward')):
            raise ValueError('Invalid terminal rule')
    sequence = task.setdefault('reset_sequence', [])
    if not isinstance(sequence, list) or len(sequence) > 64:
        raise ValueError('reset_sequence must contain at most 64 startup actions')
    for step in sequence:
        if not isinstance(step, dict) or type(step.get('frames')) is not int or not 1 <= step['frames'] <= 600 or not isinstance(step.get('buttons'), list) or not all(isinstance(b, str) and b in BUTTONS for b in step['buttons']):
            raise ValueError('Invalid reset_sequence action')
    variants = task.get('reset_variants', [[]])
    if not isinstance(variants, list) or not 1 <= len(variants) <= 14:
        raise ValueError('reset_variants must contain 1–14 startup sequences')
    for variant in variants:
        if not isinstance(variant, list) or len(variant) > 16384:
            raise ValueError('Invalid reset variant')
        for step in variant:
            if not isinstance(step, dict) or type(step.get('frames')) is not int or not 1 <= step['frames'] <= 600 or not isinstance(step.get('buttons'), list) or any(not isinstance(b, str) or b not in BUTTONS for b in step['buttons']):
                raise ValueError('Invalid reset variant action')
    requirements = task.get('evaluation_requirements', [])
    if not isinstance(requirements, list):
        raise ValueError('evaluation_requirements must be a list')
    for rule in requirements:
        if not isinstance(rule, dict) or type(rule.get('variant')) is not int or not 0 <= rule['variant'] < len(variants) or rule.get('metric') not in metric_names or not number(rule.get('min_gain')):
            raise ValueError('Evaluation requirements need a configured variant, metric and finite min_gain')
    return task

class TinyBirdEnv(gym.Env):
    metadata = {'render_modes': ['rgb_array'], 'render_fps': 60}

    def __init__(self, rom, observations, task, *, executable='tinybird-headless', bios=None, state=None, render_mode=None):
        super().__init__()
        observations = read_json(observations)
        self.task = validate_task(read_json(task), observations)
        if render_mode not in (None, 'rgb_array'):
            raise ValueError('Only rgb_array rendering is supported')
        self.render_mode = render_mode
        self.action_space = gym.spaces.Discrete(len(self.task['actions']))
        self.observation_space = gym.spaces.Box(0, 1, (len(self.task['features']) * self.task['history'],), dtype=np.float32)
        self.images = deque(maxlen=self.task['history'])
        self.cached_frame = None
        if self.task.get('visual'):
            visual = self.task['visual']
            self.observation_space = gym.spaces.Dict({'ram': self.observation_space,
                'image': gym.spaces.Box(0, 255, (self.task['history'], visual['height'], visual['width']), dtype=np.uint8)})
        self.client = TinyBird(rom, executable=executable, bios=bios, state=state, observations=observations)
        try:
            for step in self.task['reset_sequence']:
                self.client.step(step['buttons'], frames=step['frames'])
            if self.task['reset_sequence'] or self.task.get('reset_variants'):
                self.client.save_state(15)
            if self.task.get('reset_variants'):
                for index, variant in enumerate(self.task['reset_variants']):
                    self.client.load_state(15)
                    for step in variant:
                        self.client.step(step['buttons'], frames=step['frames'])
                    self.client.save_state(index)
        except Exception:
            self.client.close()
            raise
        self.history = deque(maxlen=self.task['history'])
        self.done = True
        self.closed = False

    def _features(self, memory):
        return np.asarray([np.clip((memory[f['field']] - f['min']) / (f['max'] - f['min']), 0, 1) for f in self.task['features']], dtype=np.float32)

    def _observation(self, reset=False):
        ram = np.concatenate(self.history)
        if not self.task.get('visual'):
            return ram
        from PIL import Image
        self.cached_frame = None
        visual = self.task['visual']
        pixels = np.asarray(Image.fromarray(self.render()).convert('L').resize(
            (visual['width'], visual['height']), Image.Resampling.BOX), dtype=np.uint8)
        if reset:
            self.images.clear()
            for _ in range(self.task['history']):
                self.images.append(pixels)
        else:
            self.images.append(pixels)
        return {'ram': ram, 'image': np.stack(self.images)}

    def _info(self, observation, **extra):
        memory = observation['memory']
        return {'frame': observation['frame'], 'memory': dict(memory),
                'elapsed_frames': self.elapsed_frames, 'elapsed_seconds': self.elapsed_frames / 60,
                'action_frames': self.last_action_frames,
                'metrics': {m['name']: memory[m['field']] * m.get('scale', 1) for m in self.task['metrics']},
                'out_of_range': [f['field'] for f in self.task['features'] if not f['min'] <= memory[f['field']] <= f['max']], **extra}

    def reset(self, *, seed=None, options=None):
        super().reset(seed=seed)
        if self.closed:
            raise RuntimeError('Environment is closed')
        requested_variant = None
        if options:
            if set(options) != {'variant'} or type(options['variant']) is not int or not self.task.get('reset_variants') or not 0 <= options['variant'] < len(self.task['reset_variants']):
                raise ValueError('Reset options may select a valid configured variant')
            requested_variant = options['variant']
        self.done = True
        variant = None
        if self.task.get('reset_variants'):
            variant = requested_variant if requested_variant is not None else int(self.np_random.integers(len(self.task['reset_variants'])))
            self.client.load_state(variant)
        elif self.task['reset_sequence']:
            self.client.load_state(15)
        else:
            self.client.reset()
        choice = int(self.np_random.integers(len(self.task['reset_choices'])))
        observation = self.client.step(self.task['reset_choices'][choice], frames=self.task['reset_frames'])
        self.previous = observation['memory']
        for rule in self.task.get('terminal', []):
            if OPS[rule['op']](self.previous[rule['field']], rule['value']):
                raise ValueError('Starting checkpoint already meets a terminal rule; check the state and field definitions')
        self.best = [self.previous[r['field']] * r['scale'] for r in self.task.get('rewards', [])]
        self.reward_paid = [0.0 for _ in self.best]
        self.elapsed_frames = 0
        self.last_action_frames = 0
        self.history.clear()
        for _ in range(self.task['history']):
            self.history.append(self._features(self.previous))
        self.steps = 0
        self.stall_steps = 0
        self.stall_frames = 0
        self.productive_best = {f:self.previous[f] for f in self.task.get('stall', {}).get('productive_fields', [])}
        self.stall_best = self.previous[self.task['stall']['field']] if self.task.get('stall') else None
        self.done = False
        self.cached_frame = None
        return self._observation(reset=True), self._info(observation, reset_choice=choice, reset_variant=variant)

    def step(self, action):
        if self.done or self.closed:
            raise RuntimeError('Call reset before stepping a new episode')
        if not self.action_space.contains(action):
            raise ValueError('Action is outside the action space')
        try:
            duration = self.task.get('action_frames', [self.task['frame_skip']] * self.action_space.n)[int(action)]
            remaining = self.task.get('max_frames', self.elapsed_frames + duration + self.task['release_frames']) - self.elapsed_frames
            duration = min(duration, remaining)
            observation = self.client.step(self.task['actions'][int(action)], frames=duration)
            self.last_action_frames = duration
            # An optional real release frame makes repeated jump actions new
            # presses. Merely clearing input between stdio calls advances no
            # frames, so a game otherwise sees A held continuously.
            reached_terminal = any(OPS[r['op']](observation['memory'][r['field']], r['value']) for r in self.task.get('terminal', []))
            release_frames = min(self.task['release_frames'], remaining - duration)
            if release_frames and not reached_terminal:
                held = self.task['actions'][int(action)]
                release = self.task.get('release_buttons')
                buttons = [b for b in held if b not in release] if release is not None else []
                observation = self.client.step(buttons, frames=release_frames)
                self.last_action_frames += release_frames
        except Exception:
            self.done = True  # The runtime can partially advance before reporting a stalled frame.
            raise
        memory = observation['memory']
        reward = self.task['step_reward']
        reward_terms = {'step': reward}
        self.elapsed_frames += self.last_action_frames
        if self.task.get('frame_reward'):
            reward_terms['time'] = self.last_action_frames * self.task['frame_reward']
            reward += reward_terms['time']
        for index, term in enumerate(self.task.get('rewards', [])):
            value = memory[term['field']] * term['scale']
            if term['mode'] == 'progress':
                amount = max(0, value - self.best[index])
                self.best[index] = max(self.best[index], value)
                amount = min(amount, max(0, term.get('cap', float('inf')) - self.reward_paid[index]))
                self.reward_paid[index] += amount
            else:
                amount = (memory[term['field']] - self.previous[term['field']]) * term['scale']
            reward += amount
            reward_terms[term['field']] = reward_terms.get(term['field'], 0) + amount
        outcome = None
        # Rules are ordered; put failure rules first when outcomes can overlap.
        for rule in self.task.get('terminal', []):
            if OPS[rule['op']](memory[rule['field']], rule['value']):
                reward += rule['reward']; reward_terms['terminal'] = rule['reward']; outcome = rule['outcome']; break
        self.steps += 1
        terminated = outcome is not None
        stalled = False
        if self.task.get('stall'):
            stall = self.task['stall']
            forward = memory[stall['field']] >= self.stall_best + stall['min_delta']
            productive = any(memory[f] > best for f, best in self.productive_best.items())
            for field in self.productive_best:
                self.productive_best[field] = max(self.productive_best[field], memory[field])
            if forward or productive:
                self.stall_best = max(self.stall_best, memory[stall['field']])
                self.stall_steps = 0
                self.stall_frames = 0
            else:
                self.stall_steps += 1
                self.stall_frames += self.last_action_frames
            stalled = self.stall_frames >= stall['frames'] if 'frames' in stall else self.stall_steps >= stall['steps']
        truncated = not terminated and (stalled or self.steps >= self.task['max_steps'] or self.elapsed_frames >= self.task.get('max_frames', float('inf')))
        if truncated and stalled:
            reward_terms['stall'] = self.task['stall'].get('reward', 0)
            reward += reward_terms['stall']
        self.done = terminated or truncated
        self.previous = memory
        self.history.append(self._features(memory))
        self.cached_frame = None
        return self._observation(), float(reward), terminated, truncated, self._info(observation, outcome=outcome, is_success=outcome == 'success', reward_terms=reward_terms, truncation_reason=('stalled' if stalled else 'time_limit') if truncated else None)

    def render(self):
        if self.cached_frame is None:
            frame = self.client.get_frame()
            self.cached_frame = np.asarray(frame['pixels'], dtype=np.uint8).reshape(frame['height'], frame['width'], 3)
        return self.cached_frame.copy()

    def close(self):
        if not self.closed:
            self.closed = True
            self.client.close()
