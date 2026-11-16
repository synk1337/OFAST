"""Tests of the Unreal scripts' own arithmetic, run outside Unreal (CI runs them):

    python -m unittest discover -s tools/unreal/tests

Unreal's `unreal` module only exists inside the editor, so a stand-in takes its place here: what these tests cover
must not call Unreal. This file: `replay_cameras.reduce_keys`, which thins the keys of every camera and player before
they are written to the Level Sequence.
"""

import os
import sys
import unittest
from unittest.mock import MagicMock

sys.modules.setdefault("unreal", MagicMock())
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
from replay_cameras import reduce_keys  # noqa: E402


def worst(frames, values, kept):
    """The furthest an original key sits from the straight lines between the kept keys."""
    index = {frame: i for i, frame in enumerate(frames)}
    out = 0.0
    for (fa, va), (fb, vb) in zip(kept, kept[1:]):
        for i in range(index[fa] + 1, index[fb]):
            t = (frames[i] - fa) / (fb - fa)
            out = max(out, max(abs(va[k] + (vb[k] - va[k]) * t - values[i][k]) for k in range(len(va))))
    return out


def kept_frames(frames, values, tolerance):
    return [frame for frame, _ in reduce_keys(frames, values, tolerance)]


class ReduceKeys(unittest.TestCase):
    def test_few_keys_stay_as_they_are(self):
        self.assertEqual(reduce_keys([], [], 0.1), [])
        self.assertEqual(reduce_keys([0], [(1.0,)], 0.1), [(0, (1.0,))])
        self.assertEqual(reduce_keys([0, 1], [(1.0,), (2.0,)], 0.1), [(0, (1.0,)), (1, (2.0,))])

    def test_a_straight_line_keeps_its_ends(self):
        frames = list(range(50))
        self.assertEqual(kept_frames(frames, [(0.5 * f, -f) for f in frames], 0.1), [0, 49])
        self.assertEqual(kept_frames(frames, [(3.0,)] * 50, 0.1), [0, 49])

    def test_a_step_keeps_both_its_sides(self):
        values = [(0.0,)] * 10 + [(5.0,)] * 10
        self.assertEqual(kept_frames(list(range(20)), values, 0.1), [0, 9, 10, 19])

    def test_a_kick_that_comes_back_down_is_followed(self):
        # The view after an AK-47's single shot (kick.dem, tick 981): 1.35 degrees up at once, back down over a
        # third of a second. The check before 2026-11-16 kept the kick's first key and none of the way down: every
        # key was within 0.1 of the line to the very next key, and the view stayed up.
        values = [(0.0,)] * 5 + [(-1.352 * 0.79 ** n,) for n in range(60)] + [(0.0,)] * 5
        frames = list(range(len(values)))
        kept = reduce_keys(frames, values, 0.1)
        self.assertLessEqual(worst(frames, values, kept), 0.1 + 1e-9)
        on_the_way_down = [frame for frame, _ in kept if 5 < frame < 60]
        self.assertGreaterEqual(len(on_the_way_down), 2)
        self.assertLess(len(kept), len(values) // 4)

    def test_every_dropped_key_is_within_tolerance_on_each_channel(self):
        # A slow curve on one channel while the other stands still, then the other way round.
        frames = list(range(200))
        values = [(0.002 * f * f, 0.0) if f < 100 else (20.0, 0.3 * (f - 100) ** 1.5) for f in frames]
        for tolerance in (0.05, 0.2, 1.0):
            kept = reduce_keys(frames, values, tolerance)
            self.assertEqual((kept[0][0], kept[-1][0]), (0, 199))
            self.assertLessEqual(worst(frames, values, kept), tolerance + 1e-9)

    def test_two_keys_on_one_frame_are_both_kept(self):
        values = [(0.0,), (1.0,), (4.0,), (4.0,), (4.0,)]
        self.assertEqual(reduce_keys([0, 1, 1, 2, 3], values, 0.1),
                         [(0, (0.0,)), (1, (1.0,)), (1, (4.0,)), (3, (4.0,))])

    def test_keys_between_frames(self):
        frames = [0.0, 0.5, 1.25, 2.0, 3.5]
        self.assertEqual(kept_frames(frames, [(2 * f,) for f in frames], 0.01), [0.0, 3.5])


if __name__ == "__main__":
    unittest.main()
