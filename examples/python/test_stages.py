"""Regression checks for stage progression and exact input capture."""
from copy import deepcopy
from pathlib import Path
import unittest
from unittest.mock import Mock

from advance_stage import advance, append_input, capture_clear, check_start, validate_route
from tinybird_env import read_json


class StageTest(unittest.TestCase):
    def setUp(self):
        self.route = read_json(Path(__file__).with_name('tasks') / 'mario-ax4p-stage-1-to-2.route.json')
        self.initial = {'memory': {'game.sublevel': 34, 'game.mode': 26,
                                  'player.world_x_fixed': 6144},
                        'metrics': {'score': 14100}, 'reset_choice': 0}
        self.env = Mock()
        self.env.task = {'reset_variants': [[]], 'reset_choices': [[]], 'reset_frames': 1,
                         'max_steps': 2, 'actions': [['RIGHT', 'A', 'B']],
                         'frame_skip': 16, 'release_buttons': ['A'],
                         'evaluation_requirements': [{'variant': 0, 'metric': 'score', 'min_gain': 400}]}
        self.env.reset.return_value = (None, self.initial)
        self.model = Mock()
        self.model.predict.return_value = (0, None)

    def outcome(self, success=True, score=14500, frames=17):
        return (None, 1, success, not success,
                {'is_success': success, 'metrics': {'score': score}, 'action_frames': frames})

    def test_clear_requires_success_and_new_points(self):
        for result in (self.outcome(False), self.outcome(score=14300)):
            self.env.step.return_value = result
            with self.assertRaises(RuntimeError):
                capture_clear(self.env, self.model, self.route)
        self.env.step.return_value = self.outcome()
        sequence, clear = capture_clear(self.env, self.model, self.route)
        self.assertEqual(clear['metric_gains']['score'], 400)
        self.assertEqual(sequence[-1], {'buttons': ['RIGHT', 'B'], 'frames': 1})
        self.env.reset.assert_called_with(seed=0, options={'variant': 0})

    def test_terminal_action_does_not_invent_release_frames(self):
        self.env.step.return_value = self.outcome(frames=7)
        sequence, _ = capture_clear(self.env, self.model, self.route)
        self.assertEqual(sequence, [{'buttons': [], 'frames': 1},
                                    {'buttons': ['RIGHT', 'A', 'B'], 'frames': 7}])

    def test_late_start_and_wrong_stage_are_rejected(self):
        for field, value in [('player.world_x_fixed', 500000), ('game.sublevel', 75), ('game.mode', 17)]:
            info = deepcopy(self.initial)
            info['memory'][field] = value
            with self.assertRaises(RuntimeError):
                check_start(info, self.route)

    def test_map_timeout_and_wrong_destination_are_rejected(self):
        self.env.client.step.return_value = self.initial
        with self.assertRaisesRegex(RuntimeError, 'World map'):
            advance(self.env, [], self.route)
        self.assertEqual(self.env.client.step.call_count, 30)
        map_info = deepcopy(self.initial)
        map_info['memory']['game.mode'] = 17
        self.env.client.step.side_effect = [map_info] + [self.initial] * len(self.route['sequence'])
        with self.assertRaisesRegex(RuntimeError, 'stage 75'):
            advance(self.env, [], self.route)

    def test_destination_and_input_compression(self):
        destination = deepcopy(self.initial)
        destination['memory']['game.sublevel'] = 75
        check_start(destination, self.route, destination=True)
        sequence = []
        append_input(sequence, ['RIGHT'], 599)
        append_input(sequence, ['RIGHT'], 602)
        append_input(sequence, [], 1)
        self.assertEqual([s['frames'] for s in sequence], [600, 600, 1, 1])
        self.assertEqual(sequence[-1]['buttons'], [])

    def test_route_validation(self):
        validate_route(self.route)
        for key, value in [('map_timeout_frames', 999999), ('to_stage', 34),
                           ('sequence', [{'buttons': ['BAD'], 'frames': 1}])]:
            route = deepcopy(self.route)
            route[key] = value
            with self.assertRaises(ValueError):
                validate_route(route)


if __name__ == '__main__':
    unittest.main()
