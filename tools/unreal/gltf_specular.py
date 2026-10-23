"""Unreal's Specular for the glTF materials Interchange's glTF master draws: shared by import_map.py (maps) and
import_demo.py (agents and weapons).

Interchange's glTF master (/InterchangeAssets/gltf/M_Default, and import_map.py's copy GLTF_MASTER) feeds Unreal's
Specular from its body function (MF_Default_Body): occlusion x SpecularFactor x the specular texture's alpha x MF_IOR's
((IOR - 1) / (IOR + 1))^2 / 0.08 when the material has a specular texture (static switch bHasSpecularTexture), but
occlusion x SpecularFactor alone when it has none. Unreal's F0 for non-metals is 0.08 x Specular, so glTF's defaults
(factor 1, IOR 1.5: F0 0.04) render as F0 0.08, twice what glTF means, what CS2 uses for every non-metal but weapons
(Source 2 Viewer's complex.frag.slang) and what OFAST's own masters draw (Specular unconnected, 0.5); and CS2's
weapons (KHR_materials_specular 0.5: F0 0.02) as F0 0.04 (docs/formats/lighting.md, "Reflections of Interchange's
glTF master"). Instances without a specular texture get the IOR term in their SpecularFactor instead
(specular_factor).

Interchange creates the instances with SpecularFactor 1 whatever the glTF says; the importers set it after importing.
"""

import unreal

INTERCHANGE_MASTER = "/InterchangeAssets/gltf/M_Default"
GLTF_DEFAULT_IOR = 1.5  # KHR_materials_ior's default; OFAST's exports never write the extension


def ior_specular(ior):
    """Unreal's Specular for a dielectric of index `ior` as MF_IOR computes it: F0 / 0.08, at most 1 (0.5 for 1.5)."""
    return min(((ior - 1.0) / (ior + 1.0)) ** 2 / 0.08, 1.0)


def specular_factor(material):
    """The SpecularFactor with which an instance without a specular texture reflects as the glTF material `material`
    (its JSON object) means: its KHR_materials_specular specularFactor x ior_specular(its IOR), as the master computes
    it with a texture. 0.5 (F0 0.04) by glTF's defaults, 0.25 (F0 0.02) for CS2's weapons."""
    extensions = material.get("extensions", {})
    factor = extensions.get("KHR_materials_specular", {}).get("specularFactor", 1.0)
    return factor * ior_specular(extensions.get("KHR_materials_ior", {}).get("ior", GLTF_DEFAULT_IOR))


def takes_specular_factor(instance, masters):
    """Whether `instance` needs specular_factor(): drawn by one of `masters` (path names of Interchange's glTF master
    and its copies; the unlit master has no Specular) without a specular texture (with one, the master applies the IOR
    term itself)."""
    base = instance.get_base_material()
    if base is None or base.get_path_name() not in masters:
        return False
    return not unreal.MaterialEditingLibrary.get_material_instance_static_switch_parameter_value(
        instance, "bHasSpecularTexture")
