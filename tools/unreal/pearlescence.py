"""CS2's weapon pearlescence in Unreal, used by import_demo.py: a painted weapon's colour turned in hue by how far its
surface faces away from the camera (docs/research/cs2-skins.md, "Pearlescence": csgo_weapon's combo 0).

The angle, in radians, is scale x (1 - V.N) x share: scale the kit's g_flPearlescentScale (its glTF material's
extras.ofast.pearlescence; not 0 on 64 kits, 30 of them Dopplers), V the direction to the camera (from where a
viewmodel really is, not where Unreal's first-person rendering draws it: TURN_HLSL), N the normal with
its normal map, share the painted metallic-roughness texture's R (where the paint is, by the kit's pearlescence mask:
ofast-core's paint bake). The colour turns about the grey axis (1, 1, 1)/sqrt(3) (Rodrigues), then goes back towards
its luminance (Rec. 709) by its saturation^0.125. It is the base colour, so a metal's reflections take the turn too,
as in CS2.

Interchange's glTF master (/InterchangeAssets/gltf/M_Default) has no input for it, and the plugin's content is never
edited, so MASTER is a copy of it with the turn between its body function's BaseColor and the material's (made once,
versioned like import_map.py's GLTF_MASTER: a newer engine's M_Default is picked up only by a new version). The share
is sampled again from the master's MetallicRoughnessTexture parameter, on UV channel 0, where painted textures always
are. A pearlescent material's instance moves under OFAST's copy of its Interchange parent (MI_Default_Opaque,
MI_Default_Mask…: the same blend mode and sidedness), parented to MASTER, and gets PearlescentScale; its own texture,
switch and scalar values keep their names in the copy.
"""

import unreal

import gltf_specular

FOLDER = "/Game/OFAST/Materials"
MASTER = FOLDER + "/M_OFAST_glTFPearlescent_v1"
SCALE = "PearlescentScale"

# csgo_weapon's turn, as decompiled: an angle of 0 still moves the colour towards its luminance where the kit has a
# scale (by 1 - saturation^0.125: little but for greys, which keep theirs). The view is the true one: Unreal draws a
# first-person primitive (a viewmodel) at its own field of view by moving its vertices (FirstPersonTransform, about
# the camera), which turned the camera vector away from CS2's (on woxic's Gungnir at tick 153201, half the turn);
# undone with the inverse, the camera at the origin of translated world space (as Unreal's CameraVector). The
# primitive's flag is read as IsFirstPerson does: that function comes after a Custom node's code in the shader.
TURN_HLSL = """float3 position = GetTranslatedWorldPosition(Parameters);
#if SUPPORT_FIRST_PERSON_RENDERING
if ((GetPrimitiveData(Parameters).Flags & PRIMITIVE_SCENE_DATA_FLAG_IS_FIRST_PERSON) != 0)
{
    position = mul(position, (float3x3)ResolvedView.InvFirstPersonTransform);
}
#endif
float angle = Scale * (1.0 - dot(normalize(-position), normalize(Normal))) * Share;
float c = cos(angle);
float3 axis = float3(0.57735, 0.57735, 0.57735);
float3 turned = Color * c + cross(axis, Color) * sin(angle) + axis * dot(axis, Color) * (1.0 - c);
float top = max(Color.r, max(Color.g, Color.b));
float saturation = top > 0.0 ? (top - min(Color.r, min(Color.g, Color.b))) / top : 0.0;
float luminance = dot(Color, float3(0.2125, 0.7154, 0.0721));
return lerp(luminance.xxx, turned, pow(saturation, 0.125));"""


def log(text):
    unreal.log("OFAST: " + text)


def scale(material):
    """A glTF material's (its JSON object's) pearlescence scale, 0 without."""
    return material.get("extras", {}).get("ofast", {}).get("pearlescence", 0.0)


def input_name(expression, candidates):
    """The first of `candidates` that names one of `expression`'s inputs (their names differ between versions)."""
    names = [str(n) for n in unreal.MaterialEditingLibrary.get_material_expression_input_names(expression)]
    for name in candidates:
        if name in names:
            return name
    raise RuntimeError("%s has none of the inputs %s (%s)" % (expression.get_name(), candidates, names))


def connect(source, output, target, name):
    if not unreal.MaterialEditingLibrary.connect_material_expressions(source, output, target, name):
        raise RuntimeError("%s: could not connect %s to %s" % (MASTER, source.get_name(), name))


def ensure_master():
    """MASTER, made on first use (see the module docs)."""
    assets = unreal.EditorAssetLibrary
    if assets.does_asset_exist(MASTER):
        return assets.load_asset(MASTER)
    mel = unreal.MaterialEditingLibrary
    master = assets.duplicate_asset(gltf_specular.INTERCHANGE_MASTER, MASTER)
    if master is None:
        raise RuntimeError("could not copy %s to %s" % (gltf_specular.INTERCHANGE_MASTER, MASTER))
    base_color = unreal.MaterialProperty.MP_BASE_COLOR
    body = mel.get_material_property_input_node(master, base_color)
    output = mel.get_material_property_input_node_output_name(master, base_color)
    body_inputs = dict(zip([str(n) for n in mel.get_material_expression_input_names(body)],
                           mel.get_inputs_for_material_expression(master, body)))
    surface_texture = body_inputs["MetallicRoughnessTexture"]
    x = body.get_editor_property("material_expression_editor_x") + 400
    y = body.get_editor_property("material_expression_editor_y") + 600
    uv = mel.create_material_expression(master, unreal.MaterialExpressionTextureCoordinate, x - 600, y)
    sample = mel.create_material_expression(master, unreal.MaterialExpressionTextureSample, x - 300, y)
    sample.set_editor_property("sampler_type", surface_texture.get_editor_property("sampler_type"))
    connect(surface_texture, "", sample, input_name(sample, ("Tex", "TextureObject")))
    connect(uv, "", sample, input_name(sample, ("UVs", "Coordinates")))
    amount = mel.create_material_expression(master, unreal.MaterialExpressionScalarParameter, x - 300, y + 250)
    amount.set_editor_property("parameter_name", SCALE)
    amount.set_editor_property("default_value", 0.0)
    normal = mel.create_material_expression(master, unreal.MaterialExpressionPixelNormalWS, x - 300, y + 350)
    turn = mel.create_material_expression(master, unreal.MaterialExpressionCustom, x, y)
    turn.set_editor_property("description", "CS2 pearlescence")
    turn.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT3)
    turn.set_editor_property("code", TURN_HLSL)
    custom_inputs = []
    for name in ("Color", "Scale", "Share", "Normal"):
        custom_input = unreal.CustomInput()
        custom_input.set_editor_property("input_name", name)
        custom_inputs.append(custom_input)
    turn.set_editor_property("inputs", custom_inputs)
    connect(body, output, turn, "Color")
    connect(amount, "", turn, "Scale")
    connect(sample, "R", turn, "Share")
    connect(normal, "", turn, "Normal")
    if not mel.connect_material_property(turn, "", base_color):
        raise RuntimeError("%s: could not connect the turn to the base colour" % MASTER)
    mel.recompile_material(master)
    assets.save_asset(MASTER, only_if_is_dirty=False)
    log("created %s (copy of %s, pearlescence on its base colour)" % (MASTER, gltf_specular.INTERCHANGE_MASTER))
    return master


def pearlescent_parent(parent, master):
    """OFAST's copy of Interchange's instance `parent` (MI_Default_Opaque…) under `master`, made once; `master` itself
    for M_Default."""
    assets = unreal.EditorAssetLibrary
    if not isinstance(parent, unreal.MaterialInstanceConstant):
        return master
    path = "%s/%s_Pearlescent_v1" % (FOLDER, parent.get_name())
    if assets.does_asset_exist(path):
        return assets.load_asset(path)
    copy = assets.duplicate_asset(parent.get_path_name().split(".")[0], path)
    if copy is None:
        raise RuntimeError("could not copy %s to %s" % (parent.get_path_name(), path))
    unreal.MaterialEditingLibrary.set_material_instance_parent(copy, master)
    assets.save_asset(path, only_if_is_dirty=False)
    log("created %s (copy of %s) under %s" % (path, parent.get_path_name(), MASTER))
    return copy


def make_pearlescent(found):
    """Moves the instances of pearlescent glTF materials (`found`: [(instance, its glTF material)]) under MASTER, with
    their scale. Returns how many."""
    mel = unreal.MaterialEditingLibrary
    pearlescent = [(instance, scale(material)) for instance, material in found if scale(material)]
    if not pearlescent:
        return 0
    master = ensure_master()
    for instance, value in pearlescent:
        if instance.get_base_material() != master:
            parent = pearlescent_parent(instance.get_editor_property("parent"), master)
            mel.set_material_instance_parent(instance, parent)
        mel.set_material_instance_scalar_parameter_value(instance, SCALE, value)
    return len(pearlescent)
