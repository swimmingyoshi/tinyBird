"""Optional visual-agent tests; requires requirements-training.txt."""
import copy
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import numpy as np
import torch
from stable_baselines3 import PPO
from stable_baselines3.common.env_checker import check_env

from make_training_demo import generate
from tinybird_env import TinyBirdEnv
from hybrid_policy import HybridExtractor
from demonstrations import warm_start


class HybridTest(unittest.TestCase):
    def test_value_updates_cannot_change_separate_actor(self):
        model = PPO('MultiInputPolicy', self.env, n_steps=8, batch_size=4,
                    policy_kwargs={'features_extractor_class':HybridExtractor, 'share_features_extractor':False})
        obs, _ = self.env.reset(seed=1)
        tensor, _ = model.policy.obs_to_tensor(obs)
        with torch.no_grad():
            before = model.policy.get_distribution(tensor).distribution.logits.clone()
        model.policy.optimizer.zero_grad()
        loss = (model.policy.predict_values(tensor) - 100).square().mean()
        loss.backward()
        self.assertTrue(all(p.grad is None for p in model.policy.pi_features_extractor.parameters()))
        self.assertTrue(any(p.grad is not None and p.grad.abs().sum() > 0 for p in model.policy.vf_features_extractor.parameters()))
        model.policy.optimizer.step()
        with torch.no_grad():
            after = model.policy.get_distribution(tensor).distribution.logits
        torch.testing.assert_close(before, after, rtol=0, atol=0)

    def test_expanded_action_transfer_preserves_old_weights_and_rejects_reinterpretation(self):
        import gymnasium as gym
        from train import initialize_actor
        options = dict(n_steps=8, batch_size=4, policy_kwargs={'features_extractor_class': HybridExtractor})
        source = PPO('MultiInputPolicy', self.env, **options)
        old = {k: None for k in ('rom', 'bios', 'state', 'runtime', 'observations')}
        old['observations'] = {'schema_version':1, 'fields':[{'name':'task.distance','address':0x02000000,'width':4}]}
        old['task'] = copy.deepcopy(self.env.task)
        new = copy.deepcopy(old)
        new['observations']['fields'].append({'name':'coins','address':0x02000004,'width':1})
        new['task']['actions'].append(['LEFT'])
        self.env.action_space = gym.spaces.Discrete(3)
        target_options = copy.deepcopy(options)
        target_options['policy_kwargs']['share_features_extractor'] = False
        target = PPO('MultiInputPolicy', self.env, **target_options)
        with patch('train.load_run', return_value=({'signature': old}, {}, source)):
            initialize_actor(target, 'unused', new)
            torch.testing.assert_close(target.policy.action_net.weight[:2], source.policy.action_net.weight)
            actor = next(target.policy.pi_features_extractor.parameters())
            critic = next(target.policy.vf_features_extractor.parameters())
            torch.testing.assert_close(actor, critic)
            self.assertNotEqual(actor.data_ptr(), critic.data_ptr())
            new['task']['actions'][0] = ['DOWN']
            with self.assertRaisesRegex(ValueError, 'action meanings'):
                initialize_actor(target, 'unused', new)
            new['task']['actions'][0] = old['task']['actions'][0]
            new['observations']['fields'][0]['address'] += 1
            with self.assertRaisesRegex(ValueError, 'existing observations'):
                initialize_actor(target, 'unused', new)

    def test_selection_covers_all_starts_and_prioritizes_completion(self):
        from train import evaluate, evaluate_suite, selection_rank
        result = evaluate_suite(self.env, None, 123)
        self.assertEqual([e['reset_variant'] for e in result['episodes']], [0, 1])
        self.assertTrue(all(e['elapsed_frames'] > 0 for e in result['episodes']))
        better_reward = copy.deepcopy(result)
        better_reward['mean_reward'] += 1000
        completed = copy.deepcopy(result)
        completed['success_rate'] = 1
        self.assertGreater(selection_rank(completed), selection_rank(better_reward))
        shorter_but_productive = copy.deepcopy(result)
        shorter_but_productive['mean_reward'] += 1
        for e in shorter_but_productive['episodes']:
            e['metric_gains']['world_x'] = -10
        self.assertGreater(selection_rank(shorter_but_productive), selection_rank(result))
        balanced = evaluate(self.env, None, 4, 1, stratified=True)
        self.assertEqual([e['reset_variant'] for e in balanced['episodes']], [0,1,0,1])

    def test_selection_rejects_lost_collection_behavior(self):
        from train import evaluate_suite, selection_rank
        self.env.task['metrics'] = [{'name':'score','field':'task.distance','scale':1}]
        self.env.task['evaluation_requirements'] = [{'variant':0,'metric':'score','min_gain':400}]
        failed = evaluate_suite(self.env, None, 1)
        self.assertFalse(failed['requirements_met'])
        self.env.task['evaluation_requirements'][0]['min_gain'] = -2
        self.assertTrue(evaluate_suite(self.env, None, 1)['requirements_met'])
        passed = copy.deepcopy(failed)
        passed['requirements_met'] = True
        failed['success_rate'] = 1
        failed['mean_reward'] = 1000
        self.assertGreater(selection_rank(passed), selection_rank(failed))

    def setUp(self):
        torch.set_num_threads(1)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        rom, observations, task = generate(self.directory.name)
        task.update(visual={'width':96, 'height':64}, history=4, terminal=[], max_steps=10,
                    release_frames=1, release_buttons=['A'], actions=[['RIGHT','A'], []],
                    reset_variants=[[], [{'buttons':[], 'frames':2}]])
        executable = Path(__file__).resolve().parents[2] / 'target/debug' / ('tinybird-headless.exe' if os.name == 'nt' else 'tinybird-headless')
        self.env = TinyBirdEnv(rom, observations, task, executable=executable)
        self.addCleanup(self.env.close)

    def test_visual_contract_reset_and_jump_release(self):
        check_env(self.env, warn=True)
        color = [0]
        def frame():
            return {'width':240, 'height':160, 'pixels':[color[0]] * (240*160*3)}
        with patch.object(self.env.client, 'get_frame', side_effect=frame), patch.object(self.env.client, 'step', wraps=self.env.client.step) as step:
            obs, initial = self.env.reset(seed=3)
            self.assertEqual(obs['image'].shape, (4,64,96))
            self.assertEqual(obs['image'].dtype, np.uint8)
            step.reset_mock()
            color[0] = 10
            obs, _, _, _, info = self.env.step(0)
            self.assertEqual(info['frame'] - initial['frame'], 2)
            self.assertEqual(info['memory']['task.distance'], 0)
            self.assertEqual(step.call_args_list[1].args[0], ['RIGHT'])
            color[0] = 20
            obs, *_ = self.env.step(0)
            np.testing.assert_array_equal(obs['image'][:,0,0], [0,0,10,20])
            color[0] = 30
            obs, repeated = self.env.reset(seed=3)
            self.assertEqual(repeated['reset_variant'], initial['reset_variant'])
            self.assertTrue(np.all(obs['image'] == 30))
            self.assertEqual(self.env.reset(options={'variant':1})[1]['reset_variant'], 1)
            with self.assertRaises(ValueError):
                self.env.reset(options={'variant':-1})

    def test_both_network_branches_receive_gradients_and_reload(self):
        model = PPO('MultiInputPolicy', self.env, n_steps=16, batch_size=8, seed=2,
                    policy_kwargs={'features_extractor_class':HybridExtractor, 'net_arch':[32]})
        obs, _ = self.env.reset(seed=1)
        obs['image'] = np.random.default_rng(1).integers(0,256,obs['image'].shape,dtype=np.uint8)
        tensor, _ = model.policy.obs_to_tensor(obs)
        output = model.policy.extract_features(tensor)
        output.sum().backward()
        extractor = model.policy.features_extractor
        self.assertGreater(extractor.vision[0].weight.grad.abs().sum().item(), 0)
        self.assertGreater(extractor.ram[0].weight.grad.abs().sum().item(), 0)
        changed = copy.deepcopy(obs); changed['image'][:] = 0
        other, _ = model.policy.obs_to_tensor(changed)
        self.assertFalse(torch.allclose(output, model.policy.extract_features(other)))
        model.learn(32)
        path = Path(self.directory.name) / 'model.zip'
        model.save(path)
        loaded = PPO.load(path)
        self.assertEqual(int(model.predict(obs, deterministic=True)[0]), int(loaded.predict(obs, deterministic=True)[0]))

    def test_demonstrations_are_replayed_and_failed_prefixes_rejected(self):
        model = PPO('MultiInputPolicy', self.env, n_steps=16, batch_size=8, seed=2,
                    policy_kwargs={'features_extractor_class':HybridExtractor, 'net_arch':[32]})
        report = warm_start(model, self.env, {'schema_version':1, 'label_smoothing':.05, 'episodes':[{'variant':0, 'actions':[0,0]}]}, epochs=2)
        self.assertEqual(report['transitions'], 2)
        self.assertEqual(report['label_smoothing'], .05)
        with self.assertRaisesRegex(ValueError, 'label_smoothing'):
            warm_start(model, self.env, {'schema_version':1, 'label_smoothing':1, 'episodes':[{'variant':0, 'actions':[0]}]}, epochs=2)
        with self.assertRaisesRegex(ValueError, 'failed or timed out'):
            warm_start(model, self.env, {'schema_version':1, 'episodes':[{'variant':0, 'actions':[0]*10}]}, epochs=2)


if __name__ == '__main__':
    unittest.main()
