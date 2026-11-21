"""The render settings OFAST's Unreal scripts set (`RENDER_CVARS` in import_map.py and shoot_views.py) and the ones the
project template sets from the editor's first frame (`unreal/Sandbox/Config/DefaultEngine.ini`, `[SystemSettings]`,
which the Studio installs as the user's project settings) must agree. The ini is what matters most: Lumen's diffuse
indirect off async compute keeps the realistic de_mirage from hanging the GPU in the first frames after it loads,
before any script could set it (docs/formats/lighting.md, "A GPU hang while loading").

Plain Python, run by CI with the other tests here. The scripts are read, not imported: import_map.py runs an import
when loaded.
"""

import ast
import configparser
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
UNREAL_TOOLS = os.path.join(HERE, "..")
INI = os.path.join(HERE, "..", "..", "..", "unreal", "Sandbox", "Config", "DefaultEngine.ini")


def render_cvars(script):
    """The `RENDER_CVARS` tuple of a script in tools/unreal, as {cvar: value}."""
    with open(os.path.join(UNREAL_TOOLS, script), encoding="utf-8") as f:
        tree = ast.parse(f.read())
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", None) == "RENDER_CVARS" for t in node.targets):
            return dict(command.split() for command in ast.literal_eval(node.value))
    raise AssertionError("%s has no RENDER_CVARS" % script)


def system_settings():
    """The project template's [SystemSettings], as {cvar: value} (Unreal's ini: `;` comments, repeated sections)."""
    parser = configparser.ConfigParser(strict=False, interpolation=None, comment_prefixes=(";",),
                                       inline_comment_prefixes=None, delimiters=("=",))
    parser.optionxform = str  # cvar names keep their case
    with open(INI, encoding="utf-8") as f:
        parser.read_file(f)
    return dict(parser["SystemSettings"])


class RenderCvarsTest(unittest.TestCase):
    def test_the_scripts_set_the_same(self):
        self.assertEqual(render_cvars("import_map.py"), render_cvars("shoot_views.py"))

    def test_the_project_sets_them_from_the_first_frame(self):
        ini = system_settings()
        for cvar, value in render_cvars("import_map.py").items():
            self.assertIn(cvar, ini, "%s is set by the scripts but not by DefaultEngine.ini" % cvar)
            self.assertEqual(float(ini[cvar]), float(value), cvar)

    def test_lumen_diffuse_indirect_stays_off_async_compute(self):
        # The workaround for the GPU hang on loading the realistic de_mirage (2026-11-22).
        self.assertEqual(system_settings().get("r.Lumen.DiffuseIndirect.AsyncCompute"), "0")


if __name__ == "__main__":
    unittest.main()
