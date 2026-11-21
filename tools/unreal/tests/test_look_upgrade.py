"""Tests of the Unreal scripts' own arithmetic, run outside Unreal (CI runs them; see test_reduce_keys.py for the
stand-in `unreal` module). This file: `ofast_look.upgrade`, which moves a look file written before Blur with distance
became the players' eyes' default (2026-11-21) to the new defaults, unless its blur was changed.
"""

import os
import sys
import unittest
from unittest.mock import MagicMock

sys.modules.setdefault("unreal", MagicMock())
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import ofast_look  # noqa: E402


def record(depth_blur, depth_of_field, base_dof, version=None):
    out = {"values": {"depth_blur": depth_blur, "depth_of_field": depth_of_field, "sun_height": 20.0},
           "base": {"depth_of_field": base_dof}}
    if version is not None:
        out["version"] = version
    return out


class UpgradeTest(unittest.TestCase):
    def test_a_realistic_look_at_its_old_defaults_blurs_with_distance(self):
        r = record(False, 0.81, 0.81)
        self.assertTrue(ofast_look.upgrade(r))
        self.assertTrue(r["values"]["depth_blur"])
        self.assertEqual(r["values"]["depth_of_field"], ofast_look.DEPTH_BLUR_AMOUNT)
        self.assertEqual(r["version"], ofast_look.LOOK_VERSION)
        # The user's other settings stay.
        self.assertEqual(r["values"]["sun_height"], 20.0)

    def test_cs2s_look_stays_sharp(self):
        r = record(False, 0.0, 0.0)
        self.assertTrue(ofast_look.upgrade(r))
        self.assertEqual(r["values"]["depth_of_field"], 0.0)

    def test_a_blur_the_user_set_is_kept(self):
        for depth_blur, dof in ((False, 0.4), (True, 0.81)):
            r = record(depth_blur, dof, 0.81)
            self.assertTrue(ofast_look.upgrade(r))
            self.assertEqual((r["values"]["depth_blur"], r["values"]["depth_of_field"]), (depth_blur, dof))

    def test_a_file_already_upgraded_is_left_alone(self):
        r = record(False, 0.81, 0.81, version=ofast_look.LOOK_VERSION)
        self.assertFalse(ofast_look.upgrade(r))
        self.assertFalse(r["values"]["depth_blur"])


if __name__ == "__main__":
    unittest.main()
