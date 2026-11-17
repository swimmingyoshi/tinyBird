"""Warm-start a PPO actor from verified local action demonstrations."""
import numpy as np
import torch
from torch.nn import functional as F


def warm_start(model, env, config, epochs=200):
    demos = config.get('episodes') if isinstance(config, dict) else None
    if not isinstance(config, dict) or config.get('schema_version') != 1 or not isinstance(demos, list) or not 1 <= len(demos) <= 32:
        raise ValueError('Demonstrations need schema_version 1 and 1–32 episodes')
    smoothing = config.get('label_smoothing', 0.0)
    if type(smoothing) not in (int, float) or not 0 <= smoothing <= .2:
        raise ValueError('label_smoothing must be between 0 and 0.2')
    for demo in demos:
        if not isinstance(demo, dict) or type(demo.get('variant')) is not int or not isinstance(demo.get('actions'), list) or not 1 <= len(demo['actions']) <= 2048:
            raise ValueError('Invalid demonstration episode')
        if not 0 <= demo['variant'] < len(env.unwrapped.task.get('reset_variants', [])):
            raise ValueError('Demonstration starting variant is not configured')
        if any(type(a) is not int or not env.action_space.contains(a) for a in demo['actions']):
            raise ValueError('Demonstration action is out of range')
    if sum(len(d['actions']) for d in demos) > 10000:
        raise ValueError('Demonstrations exceed 10,000 transitions')
    states, actions = [], []
    for index, demo in enumerate(demos):
        observation, _ = env.reset(seed=model.seed + index, options={'variant':demo['variant']})
        for action in demo['actions']:
            previous = observation
            observation, _, terminated, truncated, info = env.step(action)
            if info.get('outcome') == 'failure' or truncated:
                raise ValueError('A demonstration failed or timed out; use a verified surviving prefix')
            states.append(previous); actions.append(action)
            if terminated:
                break
    observations = {k:np.stack([s[k] for s in states]) for k in states[0]} if isinstance(states[0], dict) else np.stack(states)
    tensor, _ = model.policy.obs_to_tensor(observations)
    targets = torch.as_tensor(actions, device=model.device, dtype=torch.long)
    counts = torch.bincount(targets, minlength=env.action_space.n).float().clamp_min(1)
    weights = counts.rsqrt()
    rng = np.random.default_rng(model.seed)
    model.policy.set_training_mode(True)
    for _ in range(epochs):
        order = rng.permutation(len(actions))
        for start in range(0, len(actions), 32):
            indices = torch.as_tensor(order[start:start+32], device=model.device)
            batch = {k:v[indices] for k,v in tensor.items()} if isinstance(tensor, dict) else tensor[indices]
            logits = model.policy.get_distribution(batch).distribution.logits
            loss = F.cross_entropy(logits, targets[indices], weight=weights, label_smoothing=smoothing)
            model.policy.optimizer.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.policy.parameters(), .5)
            model.policy.optimizer.step()
    model.policy.set_training_mode(False)
    with torch.no_grad():
        predicted = model.policy.get_distribution(tensor).distribution.logits.argmax(dim=1)
        accuracy = (predicted == targets).float().mean().item()
    return {'transitions':len(actions), 'epochs':epochs, 'training_action_accuracy':accuracy, 'label_smoothing':smoothing}
