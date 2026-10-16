"""Debug probe: do the imported maps' Interchange materials match their current glTF?

For every glTF material of a map, the material instance Interchange made for it
(/Game/OFAST/<map>/<map>/Materials/<name>) is compared with what <map>.gltf says now:
- BaseColorTexture, MetallicRoughnessTexture, NormalTexture, OcclusionTexture, EmissiveTexture: the texture asset's
  name must be the glTF image's name (an exported PNG's name changes with its content: the exporter's file names
  carry a hash of what was baked into them), or the parameter must be unset when the glTF has no such texture;
- BaseColorFactor (and EmissiveFactor): the glTF's factor, white (black) when the glTF has none.
It also counts glTF materials without an instance, instances (and textures) no glTF material or image uses any more,
and how many mesh slots of the map's static meshes use an Interchange instance, an OFAST one or none. Lines with
"MISMATCH" list the first differences per map; the "summary" line has the counts.

Run with tools/unreal/run_ue.ps1 -Script tools/unreal/probe_materials.py -Gltf <exports folder or <map>.gltf>
-Name <map>[,<map>…]. With a folder, each map's glTF is <folder>/<map>/<map>.gltf. It only reads assets (no level
is opened, nothing is saved).
"""

import json
import os

import unreal

NAMES = [n for n in os.environ["OFAST_NAME"].split(",") if n]
GLTF = os.environ["OFAST_GLTF"]
# How many mismatches to print per map (all are counted).
SHOW = int(os.environ.get("OFAST_SHOW") or 12)
# glTF texture slots -> Interchange's parameter names (texture, factor, the factor's default).
TEXTURES = {
    "BaseColorTexture": ("pbr", "baseColorTexture"),
    "MetallicRoughnessTexture": ("pbr", "metallicRoughnessTexture"),
    "NormalTexture": ("material", "normalTexture"),
    "OcclusionTexture": ("material", "occlusionTexture"),
    "EmissiveTexture": ("material", "emissiveTexture"),
}
FACTORS = {
    "BaseColorFactor": ("pbr", "baseColorFactor", (1.0, 1.0, 1.0, 1.0)),
    "EmissiveFactor": ("material", "emissiveFactor", (0.0, 0.0, 0.0)),
}


def log(message):
    unreal.log("OFAST: " + message)


def gltf_path(name):
    if GLTF.lower().endswith(".gltf"):
        return GLTF
    return os.path.join(GLTF, name, name + ".gltf")


def expected(document, material):
    """(texture parameter -> image name or None, factor parameter -> tuple) for one glTF material."""
    pbr = material.get("pbrMetallicRoughness", {})
    sources = {"pbr": pbr, "material": material}
    textures = {}
    for param, (where, key) in TEXTURES.items():
        slot = sources[where].get(key)
        if slot is None:
            textures[param] = None
            continue
        image = document["images"][document["textures"][slot["index"]]["source"]]
        textures[param] = image.get("name") or os.path.splitext(os.path.basename(image["uri"]))[0]
    factors = {}
    for param, (where, key, default) in FACTORS.items():
        factors[param] = tuple(sources[where].get(key, default))
    return textures, factors


def instance_values(instance):
    """(texture parameter -> texture asset name, vector parameter -> rgba tuple, parent path) of an instance's
    overridden parameters."""
    textures = {}
    for value in instance.get_editor_property("texture_parameter_values"):
        texture = value.get_editor_property("parameter_value")
        name = str(value.get_editor_property("parameter_info").get_editor_property("name"))
        textures[name] = texture.get_name() if texture else None
    vectors = {}
    for value in instance.get_editor_property("vector_parameter_values"):
        color = value.get_editor_property("parameter_value")
        name = str(value.get_editor_property("parameter_info").get_editor_property("name"))
        vectors[name] = (color.r, color.g, color.b, color.a)
    parent = instance.get_editor_property("parent")
    return textures, vectors, parent.get_path_name() if parent else None


def asset_names(folder, cls_names):
    """Asset names in `folder` (not recursive) whose class is one of `cls_names`, from the asset registry (the
    assets are not loaded)."""
    registry = unreal.AssetRegistryHelpers.get_asset_registry()
    names = {}
    for data in registry.get_assets_by_path(folder, recursive=False):
        cls = str(data.asset_class_path.asset_name)
        if cls in cls_names:
            names[str(data.asset_name)] = cls
    return names


def same_factor(want, have):
    have = have[: len(want)]
    return all(abs(a - b) < 1e-3 for a, b in zip(want, have))


def probe(name):
    path = gltf_path(name)
    with open(path) as f:
        document = json.load(f)
    folder = "/Game/OFAST/%s/%s" % (name, name)
    instances = asset_names(folder + "/Materials", ("MaterialInstanceConstant", "Material"))
    textures = asset_names(folder + "/Textures", ("Texture2D",))
    images = set(i.get("name") or os.path.splitext(os.path.basename(i["uri"]))[0] for i in document.get("images", []))
    checked = missing = stale = 0
    counts = {}
    shown = 0
    gltf_names = set()
    for material in document.get("materials", []):
        material_name = material.get("name", "")
        gltf_names.add(material_name)
        asset = None
        for candidate in (material_name, "MI_" + material_name):
            if candidate in instances:
                asset = unreal.EditorAssetLibrary.load_asset(folder + "/Materials/" + candidate)
                break
        if asset is None:
            missing += 1
            continue
        checked += 1
        want_textures, want_factors = expected(document, material)
        have_textures, have_vectors, parent = instance_values(asset)
        problems = []
        for param, image in want_textures.items():
            have = have_textures.get(param)
            if have is not None and have.startswith("T_") and have[2:] == image:
                have = image
            if have != image:
                problems.append("%s %s (glTF %s)" % (param, have, image))
        for param, want in want_factors.items():
            have = have_vectors.get(param)
            default = FACTORS[param][2]
            if not same_factor(want, have if have is not None else default):
                problems.append("%s %s (glTF %s)" % (param, tuple(round(c, 3) for c in have) if have else None,
                                                     tuple(round(c, 3) for c in want)))
        if problems:
            stale += 1
            for problem in problems:
                counts[problem.split(" ")[0]] = counts.get(problem.split(" ")[0], 0) + 1
            if shown < SHOW:
                shader = material.get("extras", {}).get("ofast", {}).get("shader")
                log("%s MISMATCH %s [%s, parent %s]: %s" % (name, material_name, shader,
                                                            parent.rsplit("/", 1)[-1] if parent else None,
                                                            "; ".join(problems)))
                shown += 1
    orphan_instances = [n for n in instances if n not in gltf_names and n[3:] not in gltf_names]
    orphan_textures = [n for n in textures if n not in images and n[2:] not in images]
    log("%s summary: %d glTF materials, %d instances checked, %d stale (%s), %d without an instance; "
        "%d instances and %d of %d textures no longer in the glTF"
        % (name, len(document.get("materials", [])), checked, stale,
           ", ".join("%s %d" % kv for kv in sorted(counts.items())) or "none", missing,
           len(orphan_instances), len(orphan_textures), len(textures)))
    mesh_slots(name, folder)


def mesh_slots(name, folder):
    """Which material folder the map's static-mesh slots point to (Interchange's, OFAST's Blend/ or none)."""
    registry = unreal.AssetRegistryHelpers.get_asset_registry()
    kinds = {}
    for data in registry.get_assets_by_path(folder + "/StaticMeshes", recursive=False):
        if str(data.asset_class_path.asset_name) != "StaticMesh":
            continue
        mesh = unreal.EditorAssetLibrary.load_asset(str(data.package_name))
        for slot in mesh.static_materials:
            material = slot.material_interface
            if material is None:
                kind = "none"
            else:
                kind = material.get_path_name().rsplit("/", 1)[0].replace("/Game/OFAST/%s" % name, "<map>")
            kinds[kind] = kinds.get(kind, 0) + 1
    log("%s mesh slots: %s" % (name, ", ".join("%s %d" % kv for kv in sorted(kinds.items()))))


for map_name in NAMES:
    try:
        probe(map_name)
    except Exception as error:
        log("FAILED %s: %s" % (map_name, error))
