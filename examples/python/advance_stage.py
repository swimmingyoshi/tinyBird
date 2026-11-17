"""Unlock a training stage by replaying a verified policy clear and map route.

Checkpoints are input replays, cached by TinyBirdEnv; ROMs and save files stay local.
"""
import argparse
from copy import deepcopy
import json
from pathlib import Path
import sys

import torch

from tinybird_env import BUTTONS, TinyBirdEnv, read_json, validate_task
from train import digest, load_run


def validate_route(route):
    required = {'schema_version', 'from_stage', 'to_stage', 'stage_field',
                'mode_field', 'playing_mode', 'map_mode', 'start_field',
                'max_start_value', 'map_timeout_frames', 'sequence'}
    if not isinstance(route, dict) or set(route) != required or type(route['schema_version']) is not int or route['schema_version'] != 1:
        raise ValueError('Unsupported stage route')
    for name in ('from_stage', 'to_stage', 'playing_mode', 'map_mode',
                 'max_start_value', 'map_timeout_frames'):
        if type(route[name]) is not int or route[name] < 0:
            raise ValueError(f'Invalid route {name}')
    if route['from_stage'] == route['to_stage'] or not 1 <= route['map_timeout_frames'] <= 3600:
        raise ValueError('Route must advance to another stage within 3600 map frames')
    field = route['stage_field']
    if (not isinstance(field, dict) or set(field) != {'name', 'address', 'width'}
            or not isinstance(field['name'], str) or type(field['address']) is not int
            or not 0 <= field['address'] <= 0xFFFFFFFF
            or type(field['width']) is not int or field['width'] not in (1, 2, 4)):
        raise ValueError('Invalid stage observation field')
    if any(not isinstance(route[name], str) or not route[name] for name in ('mode_field', 'start_field')):
        raise ValueError('Route fields must be observation names')
    sequence = route['sequence']
    if not isinstance(sequence, list) or not 1 <= len(sequence) <= 64:
        raise ValueError('Route requires 1–64 input steps')
    for step in sequence:
        if (not isinstance(step, dict) or set(step) != {'buttons', 'frames'}
                or type(step['frames']) is not int or not 1 <= step['frames'] <= 600
                or not isinstance(step['buttons'], list)
                or any(not isinstance(b, str) or b not in BUTTONS for b in step['buttons'])):
            raise ValueError('Invalid route input')


def append_input(sequence, buttons, frames):
    """Compress consecutive identical inputs without changing any emulated frames."""
    while frames:
        if sequence and sequence[-1]['buttons'] == buttons and sequence[-1]['frames'] < 600:
            count = min(frames, 600 - sequence[-1]['frames'])
            sequence[-1]['frames'] += count
        else:
            count = min(frames, 600)
            sequence.append({'buttons': list(buttons), 'frames': count})
        frames -= count


def check_start(info, route, destination=False):
    memory = info['memory']
    expected = route['to_stage' if destination else 'from_stage']
    if (memory[route['stage_field']['name']] != expected
            or memory[route['mode_field']] != route['playing_mode']
            or memory[route['start_field']] > route['max_start_value']):
        raise RuntimeError(f'Expected the full beginning of stage {expected}; refusing unlock')


def capture_clear(env, model, route):
    """Never unlock from a late checkpoint, death, timeout, or insufficient points."""
    observation, initial = env.reset(seed=0, options={'variant': 0})
    check_start(initial, route)
    sequence = []
    for step in env.task['reset_variants'][0]:
        append_input(sequence, **step)
    append_input(sequence, env.task['reset_choices'][initial['reset_choice']], env.task['reset_frames'])
    for _ in range(env.task['max_steps']):
        action = int(model.predict(observation, deterministic=True)[0])
        observation, _, terminated, truncated, info = env.step(action)
        buttons = env.task['actions'][action]
        duration = env.task.get('action_frames', [env.task['frame_skip']] * len(env.task['actions']))[action]
        held = min(duration, info['action_frames'])
        append_input(sequence, buttons, held)
        release = env.task.get('release_buttons')
        released = [b for b in buttons if b not in release] if release else []
        append_input(sequence, released, info['action_frames'] - held)
        if terminated or truncated:
            break
    else:
        raise RuntimeError('Stage verification exceeded the step limit')
    if not info.get('is_success'):
        raise RuntimeError('Model did not clear this stage; the next stage remains locked')
    gains = {key: value - initial['metrics'][key] for key, value in info['metrics'].items()}
    for requirement in env.task.get('evaluation_requirements', []):
        if requirement['variant'] == 0 and gains[requirement['metric']] < requirement['min_gain']:
            raise RuntimeError('Clear missed the full-stage points requirement; next stage remains locked')
    return sequence, dict(info, metric_gains=gains)


def advance(env, sequence, route):
    remaining = route['map_timeout_frames']
    while remaining:
        frames = min(60, remaining)
        state = env.client.step([], frames=frames)
        append_input(sequence, [], frames)
        remaining -= frames
        if state['memory'][route['mode_field']] == route['map_mode']:
            break
    else:
        raise RuntimeError('World map did not appear; next stage remains locked')
    for step in route['sequence']:
        state = env.client.step(**step)
        append_input(sequence, **step)
    check_start(state, route, destination=True)
    return state


def prepare(run, route, output):
    validate_route(route)
    output = Path(output).resolve()
    if output.exists():
        raise ValueError('Use a new output directory; existing checkpoints are never overwritten')
    manifest, kwargs, model = load_run(run)
    kwargs = deepcopy(kwargs)
    fields = kwargs['observations']['fields']
    existing = next((f for f in fields if f['name'] == route['stage_field']['name']), None)
    if existing is not None and existing != route['stage_field']:
        raise ValueError('Stage field conflicts with the source observations')
    if existing is None:
        fields.append(route['stage_field'])
    kwargs['task'].setdefault('reset_variants', [[]])
    print('Verifying the source model from the full stage beginning...', flush=True)
    with_env = TinyBirdEnv(**kwargs)
    try:
        sequence, clear = capture_clear(with_env, model, route)
        print(f"Verified clear: gains {clear['metric_gains']}, {clear['elapsed_seconds']:.2f}s", flush=True)
        advance(with_env, sequence, route)
    finally:
        with_env.close()
    task = kwargs['task']
    task['reset_variants'] = [sequence]
    task['reset_choices'] = [[]]
    task['reset_frames'] = 1
    # Floors belong to a particular stage. Carry rewards, not its already-earned score.
    task.pop('evaluation_requirements', None)
    validate_task(task, kwargs['observations'])
    print('Checking that the next-stage checkpoint replays and resets exactly...', flush=True)
    verifier = TinyBirdEnv(**kwargs)
    try:
        _, first = verifier.reset(options={'variant': 0})
        check_start(first, route, destination=True)
        frame = verifier.render().copy()
        verifier.step(0)
        _, second = verifier.reset(options={'variant': 0})
        if first['memory'] != second['memory'] or not (frame == verifier.render()).all():
            raise RuntimeError('Next-stage checkpoint reset was not reproducible')
    finally:
        verifier.close()
    output.mkdir(parents=True)
    def write(name, value):
        (output / name).write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
    write('task.json', task)
    write('observations.json', kwargs['observations'])
    write('unlock.json', {'schema_version': 1, 'route': route,
                         'source_run': str(Path(run).resolve()),
                         'source_model_sha256': digest(Path(run) / 'model.zip'),
                         'source_signature': manifest['signature'], 'clear': clear,
                         'next_start': first, 'task_sha256': digest(output / 'task.json'),
                         'observations_sha256': digest(output / 'observations.json')})
    from PIL import Image
    Image.fromarray(frame).save(output / 'start.png')
    # Quote literal paths for PowerShell, including spaces and apostrophes.
    quote = lambda value: "'" + str(value).replace("'", "''") + "'"
    args = [sys.executable, Path(__file__).with_name('train.py').resolve(), kwargs['rom'],
            output / 'observations.json', output / 'task.json', '--executable', kwargs['executable']]
    for name in ('state', 'bios'):
        if kwargs.get(name):
            args += ['--' + name, kwargs[name]]
    args += ['--initialize-from', Path(run).resolve(), '--steps', '4096', '--device', 'auto',
             '--eval-episodes', '4', '--output', output.parent / (output.name + '-v1')]
    command = '& ' + ' '.join(map(quote, args))
    (output / 'train.ps1').write_text(command + '\n', encoding='utf-8')
    print(f'Unlocked stage {route["to_stage"]}: {output}\nTrain with: & {quote(output / "train.ps1")}', flush=True)
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', required=True, type=Path, help='Trusted local trained run with model.zip')
    parser.add_argument('--route', required=True, type=Path, help='Verified map navigation and stage identifiers')
    parser.add_argument('--output', required=True, type=Path, help='New unlocked-stage directory')
    args = parser.parse_args()
    torch.set_num_threads(1)
    try:
        prepare(args.run, read_json(args.route), args.output)
    except (ValueError, RuntimeError, OSError) as error:
        parser.exit(1, f'Stage unlock stopped: {error}\n')


if __name__ == '__main__':
    main()
