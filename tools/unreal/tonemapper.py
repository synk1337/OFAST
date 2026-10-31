"""OFAST's replacement tone mapper for CS2's look: CS2's tone curve and colour-correction table, then the Look panel's
grading (ofast_look.py: warmth, saturation, contrast, vignette, film grain; neutral by default, so the picture is
CS2's). Used by import_map.py (each map's instance, cs2_tonemapper_instance) and ofast_look.py (the grading, and
older maps' instances moved onto this version).

Unreal applies nothing after a replacing tone mapper: its own white balance, saturation, contrast, vignette and grain
(applied in its tone mapper) do nothing in CS2's look, so the panel's grading is done here.
"""

import os

import unreal

# Versioned like import_map.py's BLEND_MASTER: v3 had no grading.
TONEMAPPER = "/Game/OFAST/Materials/M_OFAST_Tonemap_v4"
# The identity colour table's folder (import_map.py's default textures).
TEXTURE_FOLDER = "/Game/OFAST/Materials/Defaults"
# Hable maps: shown = LUT(srgb(H(INPUT x) / H(WHITE W))), x = exposure x scene (what Unreal shows linearly with the
# manual exposure above), W the .vpost white point, LUT the .vpost's colour-correction table. Measured on CS2's
# default picture (brightness r_fullscreen_gamma 2.2; docs/formats/lighting.md) with tools/compare/tone_fit.py:
# exposure pairs, every pixel through the table and the curve, fix the white at 4.1 – 4.2 W on de_anubis (both
# spawns), 3.7 – 5.6 W on de_vertigo and 3.2 – 3.4 W on de_inferno; at 4.2 W the sky pixels of known radiance give the
# same INPUT at every exposure, 2.84 on de_anubis (its visible sky is the sky that lights it) and 2.86 – 2.93 on
# de_vertigo. The previous values (3.4, 6) were measured on shots taken with this machine's brightness setting (2.013),
# which raises CS2's whole picture to the power 0.915; the one before (H(2.5 x) / H(1.25 W), no table) was fitted on
# de_inferno's surfaces.
HABLE_INPUT_SCALE = 2.85
HABLE_WHITE_SCALE = 4.2
TONEMAP_SCALES = {"InputScale": HABLE_INPUT_SCALE, "WhiteScale": HABLE_WHITE_SCALE}
HABLE_PARAMETERS = {  # material parameter -> .vpost tone map key (manifest post.tonemap)
    "ShoulderStrength": "shoulder_strength",
    "LinearStrength": "linear_strength",
    "LinearAngle": "linear_angle",
    "ToeStrength": "toe_strength",
    "ToeNumerator": "toe_numerator",
    "ToeDenominator": "toe_denominator",
    "WhitePoint": "white_point",
}
# Switches and table size, set per map (cs2_tonemapper_instance).
TONEMAP_SWITCHES = {"Hable": 1.0, "UseLut": 0.0, "LutSize": 32.0}
# The Look panel's grading and their neutral values: Warmth (−1 cool … 1 warm), Saturation and Contrast (times),
# Vignette (0 – 1: how dark the corners get), Grain (0 – 1).
GRADING = {"Warmth": 0.0, "Saturation": 1.0, "Contrast": 1.0, "Vignette": 0.0, "Grain": 0.0}
TONEMAP_HLSL = """
float3 x = max(Color * Exposure, 0.0);
float3 c;
if (Hable > 0.5)
{
    x *= InputScale;
    float A = ShoulderStrength, B = LinearStrength, C = LinearAngle, D = ToeStrength, E = ToeNumerator, F = ToeDenominator;
    float3 h = (x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F) - E / F;
    float w = WhitePoint * WhiteScale;
    float hw = (w * (A * w + C * B) + D * E) / (w * (A * w + B) + D * F) - E / F;
    c = saturate(h / hw);
}
else
{
    c = saturate(x);
}
// The Look panel's grading, on the curve's linear picture (all neutral by default): warmth, saturation, contrast round
// mid-grey, the corners darkened by the vignette.
c *= float3(1.0 + 0.12 * Warmth, 1.0, 1.0 - 0.12 * Warmth);
float luma = dot(c, float3(0.2126, 0.7152, 0.0722));
c = max(lerp(luma.xxx, c, Saturation), 0.0);
c = 0.18 * pow(max(c, 1e-6) / 0.18, Contrast);
float2 fromCentre = (UV - 0.5) * 1.4142;
c *= 1.0 - Vignette * 0.8 * saturate(dot(fromCentre, fromCentre));
c = saturate(c);
// sRGB encoding (lerp/step instead of a vector ?: so it compiles under HLSL 2021 too).
float3 s = lerp(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, step(c, 0.0031308));
if (UseLut > 0.5)
{
    // CS2's 3D table, stored as LutSize blue slices side by side (pixel (b * n + r, g)): trilinear lookup as two
    // bilinear ones on neighbouring slices, at texel centres so a slice never bleeds into the next.
    float n = LutSize;
    float3 p = s * (n - 1.0);
    float b0 = floor(p.b);
    float b1 = min(b0 + 1.0, n - 1.0);
    float v = (p.g + 0.5) / n;
    float3 l0 = Texture2DSampleLevel(Lut, LutSampler, float2((b0 * n + p.r + 0.5) / (n * n), v), 0).rgb;
    float3 l1 = Texture2DSampleLevel(Lut, LutSampler, float2((b1 * n + p.r + 0.5) / (n * n), v), 0).rgb;
    s = lerp(l0, l1, p.b - b0);
}
// Film grain on the encoded picture: a fresh noise every frame, strongest in the mid-tones.
float noise = frac(sin(dot(floor(UV * 2048.0) + frac(Time * 61.0) * 113.0, float2(12.9898, 78.233))) * 43758.5453);
s += (noise - 0.5) * Grain * 0.12 * (1.0 - abs(s * 2.0 - 1.0));
return saturate(s);
"""
LUT_TEXTURE_SETTINGS = {
    # Uncompressed RGBA8, no sRGB decoding, no mips, bilinear, clamped: the table's bytes as they are.
    "compression_settings": unreal.TextureCompressionSettings.TC_VECTOR_DISPLACEMENTMAP,
    "srgb": False,
    "mip_gen_settings": unreal.TextureMipGenSettings.TMGS_NO_MIPMAPS,
    "lod_group": unreal.TextureGroup.TEXTUREGROUP_COLOR_LOOKUP_TABLE,
    "filter": unreal.TextureFilter.TF_BILINEAR,
    "address_x": unreal.TextureAddress.TA_CLAMP,
    "address_y": unreal.TextureAddress.TA_CLAMP,
    "never_stream": True,
}


def log(message):
    unreal.log("OFAST: " + message)


def identity_lut():
    """A 32^3 identity table in the same layout, the Lut parameter's default (maps without a table switch it off)."""
    import struct
    import tempfile
    import zlib

    n = 32
    rows = []
    for g in range(n):
        row = bytearray(b"\x00")  # PNG filter type: none
        for b in range(n):
            for r in range(n):
                row += bytes((round(r * 255 / (n - 1)), round(g * 255 / (n - 1)), round(b * 255 / (n - 1)), 255))
        rows.append(bytes(row))
    chunk = lambda kind, data: (struct.pack(">I", len(data)) + kind + data
                                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))
    path = os.path.join(tempfile.mkdtemp(), "T_OFAST_IdentityLut.png")
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n * n, n, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b""))
    asset = TEXTURE_FOLDER + "/T_OFAST_IdentityLut"
    if not unreal.EditorAssetLibrary.does_asset_exist(asset):
        task = unreal.AssetImportTask()
        task.filename = path
        task.destination_path = TEXTURE_FOLDER
        task.automated = True
        task.replace_existing = True
        task.save = False
        unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    texture = unreal.EditorAssetLibrary.load_asset(asset)
    texture.set_editor_properties(LUT_TEXTURE_SETTINGS)
    unreal.EditorAssetLibrary.save_loaded_asset(texture)
    return texture


def ensure_tonemapper():
    """Post-process material that replaces Unreal's tone mapper with CS2's picture: scene colour x exposure (Unreal's
    eye adaptation value, the manual exposure) -> straight line or H(InputScale x) / H(WhiteScale W) per channel ->
    the Look panel's grading -> sRGB -> CS2's colour-correction table -> film grain. Unreal applies nothing after a
    replacing tone mapper, so the sRGB encoding is done here."""
    if unreal.EditorAssetLibrary.does_asset_exist(TONEMAPPER):
        return unreal.EditorAssetLibrary.load_asset(TONEMAPPER)
    folder, name = TONEMAPPER.rsplit("/", 1)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew())
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_POST_PROCESS)
    material.set_editor_property("blendable_location", unreal.BlendableLocation.BL_REPLACING_TONEMAPPER)
    mel = unreal.MaterialEditingLibrary
    scene = mel.create_material_expression(material, unreal.MaterialExpressionSceneTexture, -900, 0)
    scene.set_editor_property("scene_texture_id", unreal.SceneTextureId.PPI_POST_PROCESS_INPUT0)
    exposure = mel.create_material_expression(material, unreal.MaterialExpressionEyeAdaptation, -900, 200)
    lut = mel.create_material_expression(material, unreal.MaterialExpressionTextureObjectParameter, -900, 300)
    lut.set_editor_property("parameter_name", "Lut")
    lut.set_editor_property("texture", identity_lut())
    lut.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_LINEAR_COLOR)
    uv = mel.create_material_expression(material, unreal.MaterialExpressionScreenPosition, -900, 380)
    time = mel.create_material_expression(material, unreal.MaterialExpressionTime, -900, 420)
    curve = mel.create_material_expression(material, unreal.MaterialExpressionCustom, -400, 0)
    curve.set_editor_property("description", "CS2 tone curve, colour table and the Look panel's grading")
    curve.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT3)
    curve.set_editor_property("code", TONEMAP_HLSL)
    scalars = list(TONEMAP_SCALES) + list(HABLE_PARAMETERS) + list(TONEMAP_SWITCHES) + list(GRADING)
    inputs = []
    for input_name in ["Color", "Exposure", "Lut", "UV", "Time"] + scalars:
        custom_input = unreal.CustomInput()
        custom_input.set_editor_property("input_name", input_name)
        inputs.append(custom_input)
    curve.set_editor_property("inputs", inputs)
    connected = (mel.connect_material_expressions(scene, "Color", curve, "Color")
                 and mel.connect_material_expressions(exposure, "", curve, "Exposure")
                 and mel.connect_material_expressions(lut, "", curve, "Lut")
                 and mel.connect_material_expressions(uv, "ViewportUV", curve, "UV")
                 and mel.connect_material_expressions(time, "", curve, "Time"))
    if not connected:
        raise RuntimeError("tone mapper: could not connect its inputs")
    defaults = {**TONEMAP_SCALES, **TONEMAP_SWITCHES, **GRADING}
    for index, param in enumerate(scalars):
        scalar = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900,
                                                500 + index * 110)
        scalar.set_editor_property("parameter_name", param)
        scalar.set_editor_property("default_value", defaults.get(param, 1.0))
        mel.connect_material_expressions(scalar, "", curve, param)
    mel.connect_material_property(curve, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_asset(TONEMAPPER)
    log("created " + TONEMAPPER)
    return material
