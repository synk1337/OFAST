"""Tests of the Unreal scripts' own arithmetic, run outside Unreal (CI runs them; see test_reduce_keys.py for the
stand-in `unreal` module). This file: grenade_effects.bias_power, Source's Bias(x, b) as the power the sprite
material raises a particle's age to (a layer's `scale_bias`: CS2's C_OP_InterpolateRadius `m_flBias`).
"""

import math
import os
import sys
import unittest
from unittest.mock import MagicMock

sys.modules.setdefault("unreal", MagicMock())
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import grenade_effects  # noqa: E402
import shot_effects  # noqa: E402


class BiasTest(unittest.TestCase):
    def test_half_is_linear(self):
        self.assertAlmostEqual(grenade_effects.bias_power(0.5), 1.0)
        self.assertEqual(grenade_effects.option({}, "scale_bias"), 0.5)

    def test_sources_bias_at_a_half_is_the_bias(self):
        # Source's Bias(0.5, b) = b, whatever b.
        for b in (0.2, 0.8, 0.95):
            self.assertAlmostEqual(0.5 ** grenade_effects.bias_power(b), b)

    def test_the_awp_flame_is_nearly_grown_a_tenth_into_its_life(self):
        flame = shot_effects.SNIPER_FLAME
        start, end = flame["scale"]
        grown = start + (end - start) * 0.1 ** grenade_effects.bias_power(flame["scale_bias"])
        self.assertGreater(grown, 0.9)
        self.assertTrue(math.isclose(grenade_effects.bias_power(0.8), 0.3219, abs_tol=1e-3))


if __name__ == "__main__":
    unittest.main()
