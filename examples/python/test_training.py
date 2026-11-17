"""Real runtime tests for the optional Gymnasium training adapter."""
import copy
import os
from pathlib import Path
import tempfile
import unittest

import numpy as np
from gymnasium.utils.env_checker import check_env

from make_training_demo import generate
from tinybird_env import TinyBirdEnv, validate_task

class TrainingTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.rom, self.observations, self.task = generate(self.directory.name)
        self.executable = Path(__file__).resolve().parents[2] / 'target/debug' / ('tinybird-headless.exe' if os.name == 'nt' else 'tinybird-headless')

    def env(self, task=None):
        env = TinyBirdEnv(self.rom, self.observations, task or self.task, executable=self.executable, render_mode='rgb_array')
        self.addCleanup(env.close)
        return env

    def test_contract_replay_and_success(self):
        env = self.env()
        check_env(env, skip_render_check=True)
        initial, info = env.reset(seed=17)
        self.assertEqual(info['memory']['task.distance'], 1)
        with self.assertRaises(ValueError): env.step(99)
        self.assertEqual(env.client.get_state()['frame'], info['frame'])
        observation, reward, terminated, truncated, info = env.step(2)
        self.assertEqual(info['memory']['task.distance'], 0)
        self.assertTrue(terminated); self.assertFalse(truncated)
        self.assertGreater(reward, 1)
        self.assertEqual(env.render().shape, (160, 240, 3))
        with self.assertRaises(RuntimeError): env.step(0)
        np.testing.assert_array_equal(env.reset(seed=17)[0], initial)
        env.close(); env.close()

    def test_timeout_and_progress_cannot_be_farmed(self):
        task = copy.deepcopy(self.task)
        task['terminal'] = []; task['max_steps'] = 3
        env = self.env(task); env.reset(seed=1)
        first = env.step(2)
        self.assertAlmostEqual(first[1], .99)
        self.assertAlmostEqual(env.step(1)[1], -.01)
        last = env.step(2)
        self.assertAlmostEqual(last[1], -.01)
        self.assertFalse(last[2]); self.assertTrue(last[3])

    def test_invalid_task_and_terminal_baseline(self):
        for key, value in [('frame_skip', 0), ('history', 0), ('step_reward', float('nan')), ('actions', [['NOPE']])]:
            task = copy.deepcopy(self.task); task[key] = value
            with self.assertRaises(ValueError): validate_task(task, self.observations)
        task = copy.deepcopy(self.task); task['reset_choices'] = [['RIGHT']]
        with self.assertRaisesRegex(ValueError, 'already meets'):
            self.env(task).reset()

    def test_elapsed_time_budget_and_capped_rewards(self):
        task = copy.deepcopy(self.task)
        task.update(terminal=[], max_steps=100, action_frames=[3] * len(task['actions']),
                    release_frames=1, max_frames=6, frame_reward=-.1, step_reward=0)
        for reward in task['rewards']:
            reward.update(mode='progress', cap=.25)
        env = self.env(task)
        env.reset(seed=1)
        first = env.step(0)
        self.assertEqual(first[4]['elapsed_frames'], 4)
        self.assertAlmostEqual(first[4]['reward_terms']['time'], -.4)
        last = env.step(2)
        self.assertTrue(last[3])
        self.assertEqual(last[4]['elapsed_frames'], 6)
        self.assertEqual(last[4]['action_frames'], 2)
        self.assertAlmostEqual(last[4]['elapsed_seconds'], .1)
        self.assertLessEqual(sum(env.reward_paid), .25 * len(task['rewards']))
        self.assertEqual(env.reset(seed=1)[1]['elapsed_frames'], 0)
        for key, value in [('action_frames', [0]), ('max_frames', 0), ('frame_reward', float('inf'))]:
            invalid = copy.deepcopy(task); invalid[key] = value
            with self.assertRaises(ValueError): validate_task(invalid, self.observations)

    def test_release_frames_advance_input_but_preserve_terminal_event(self):
        task = copy.deepcopy(self.task); task['release_frames'] = 1
        env = self.env(task)
        _, initial = env.reset(seed=1)
        _, _, terminated, _, waiting = env.step(0)
        self.assertEqual(waiting['frame'], initial['frame'] + 2)
        self.assertFalse(terminated)
        _, _, terminated, _, success = env.step(2)
        self.assertEqual(success['frame'], waiting['frame'] + 1)
        self.assertEqual(success['memory']['task.distance'], 0)
        self.assertTrue(terminated)

    def test_score_reward_is_incremental_and_resets_per_episode(self):
        task = copy.deepcopy(self.task)
        task.update(terminal=[], max_steps=10, reset_choices=[['RIGHT']],
                    rewards=[{'field':'task.distance', 'mode':'progress', 'scale':.025}],
                    metrics=[{'name':'score', 'field':'task.distance', 'scale':10}])
        env = self.env(task)
        self.assertEqual(env.reset()[1]['metrics']['score'], 0)
        _, reward, _, _, info = env.step(0)
        self.assertEqual(info['metrics']['score'], 10)
        self.assertAlmostEqual(info['reward_terms']['task.distance'], .025)
        self.assertAlmostEqual(sum(info['reward_terms'].values()), reward)
        self.assertAlmostEqual(env.step(0)[4]['reward_terms']['task.distance'], 0)
        env.step(2)
        self.assertAlmostEqual(env.step(0)[4]['reward_terms']['task.distance'], 0)
        env.reset()
        self.assertAlmostEqual(env.step(0)[4]['reward_terms']['task.distance'], .025)

    def test_stall_timeout_and_terminal_priority(self):
        task = copy.deepcopy(self.task)
        task['stall'] = {'field':'task.distance', 'steps':2, 'min_delta':1, 'reward':-5}
        env = self.env(task); env.reset()
        self.assertFalse(env.step(0)[3])
        _, _, terminated, truncated, info = env.step(0)
        self.assertFalse(terminated); self.assertTrue(truncated)
        self.assertEqual(info['truncation_reason'], 'stalled')
        self.assertEqual(info['reward_terms']['stall'], -5)
        env.reset(); env.step(0)
        _, _, terminated, truncated, info = env.step(2)
        self.assertTrue(terminated); self.assertFalse(truncated)
        self.assertIsNone(info['truncation_reason'])
        self.assertNotIn('stall', info['reward_terms'])

    def test_productive_pause_uses_new_points_and_emulated_time(self):
        task = copy.deepcopy(self.task)
        task.update(terminal=[], max_steps=20, reset_choices=[['RIGHT']],
                    stall={'field':'task.distance', 'min_delta':100, 'steps':1,
                           'frames':3, 'productive_fields':['task.distance'], 'reward':-5})
        env = self.env(task); env.reset()
        self.assertFalse(env.step(0)[3])  # New score despite no meaningful forward progress.
        self.assertEqual(env.stall_frames, 0)
        self.assertFalse(env.step(2)[3])
        self.assertFalse(env.step(0)[3])  # Recovering old points is not new productivity.
        self.assertEqual(env.stall_frames, 2)
        self.assertTrue(env.step(0)[3])
        env.reset()
        self.assertEqual(env.stall_frames, 0)
        self.assertFalse(env.step(0)[3])

if __name__ == '__main__':
    unittest.main()
