# 014 — Unreal lights draw a highlight only where CS2 does

- **Date:** 2026-10-18
- **Status:** accepted

## Context

Against CS2's default picture, de_mirage's sunlit surfaces read 3 – 10 % brighter in Unreal in all seven outdoor
views, and de_anubis's base colours 8 % above CS2's `mat_fullbright` albedo. With the direct sun the same formula in
both engines, this was taken for base colours 7 % too bright ([lighting.md](../formats/lighting.md#against-cs2s-default-picture-2026-10-18)).
Stage by stage ([lighting.md](../formats/lighting.md#highlights-cs2-does-not-draw-2026-10-18)):

- The exported colour textures average what CS2 stored as their average (348 de_mirage textures, median 1.0065).
- de_anubis's base colours match CS2's albedo (0.99 / 0.97) once Unreal's base-colour render is taken with
  `ShowFlag.Specular 0`: `ShowFlag.Lighting 0` alone adds 0.45 × the specular colour.
- Unreal's sun drew a highlight that CS2 does not draw. de_mirage's `light_environment` has `directlight` 1
  (baked), like nearly every official map's; a baked light lives in the lightmaps, which carry no highlight, and its
  highlight exists only if Hammer bakes it into the cubemaps (`bakespeculartocubemaps`, local lights only). With
  the sun's Specular Scale at 0 the sunlit surfaces read 0.97 – 1.04 (pooled 1.07 → 1.01), and over 5800 sunlit
  blocks Unreal / CS2 had grown with the highlight's share exactly as if CS2 had none (slope 0.92; −0.08 without).

All of de_mirage's 66 baked local lights draw no highlight in CS2 either; Unreal gave every light one.

## Decision

The exporter writes how CS2 draws each light's highlight into the manifest: the sun's `direct` (`directlight`), and
each local light's `bake_specular` / `bake_specular_scale` (`bakespeculartocubemaps`, `…_scale`) next to its
`direct`. `import_map.py` (`cs2_specular_scale`) sets each Unreal light's Specular Scale: 1 for stationary and
dynamic lights (2, 3), the bake scale for a baked light whose highlight is baked into the cubemaps, 0 for other
baked lights and bounce-only ones. Manifests without the keys keep Unreal's highlights, with a warning. The
realistic style keeps physical highlights. Base-colour renders are taken with `ShowFlag.Lighting 0` and
`ShowFlag.Specular 0`.

## Alternatives considered

- Darken the base colours or lower the sun by 7 % — wrong where it matters: the materials and the sun's formula are
  right, and the highlight's share varies with view and surface (2 – 12 %), so a constant cannot reproduce it.
- Lower the bounce gains — the shade would end 10 – 15 % dark relative to the sun.
- Rebuild CS2's cubemap highlights for baked lights with `bakespeculartocubemaps` exactly (a highlight shape seen
  only through the cubemaps) — approximated by the light's own highlight at the bake scale; none on de_mirage.

## Consequences

- de_mirage: sunlit 1.01 of CS2 pooled (0.96 – 1.04 per view), CT / T spawns 1.02 / 1.04, palace 1.01; relative to
  the sun the calibration light holds (CT shade 0.97, T 1.05, palace 1.00), so the bounce gains stay. Lamp-lit rooms
  lose their highlights too (B apartments 0.84 → 0.75).
- Other maps need a re-export for the new manifest keys; until then they keep Unreal's highlights. de_cache's sun is
  stationary and keeps its highlight; de_nuke, de_inferno and de_vertigo bake many local highlights into cubemaps.
  Done for de_anubis, de_dust2, de_inferno, de_nuke and de_vertigo (2026-10-18): sunlit surfaces 4 – 11 % down
  (de_anubis 1.06 → 1.01 of CS2), shade 0 – 4 %, de_nuke's rooms, which keep their cubemap highlights, 0 – 1 %
  ([checks.md](../checks.md#the-highlight-rule-on-the-other-maps-2026-10-18)). Those imports still had
  Interchange's F0 at 0.08 (next point).
- Interchange's glTF master reflected with F0 0.08 where CS2 uses 0.04; `import_map.py` now sets its instances'
  `SpecularFactor` to 0.5 (`set_gltf_specular`, 2026-10-18): de_mirage's sunlit surfaces pool at 1.00, and relative
  to the sun the light is CT shade 0.94, T shade 1.02, palace 0.97 (from 0.97 / 1.05 / 1.00). The bounce gains were
  not refitted ([lighting.md](../formats/lighting.md#reflections-of-interchanges-gltf-master-2026-10-18)). Measured
  on de_mirage only; the other five maps were imported before it.
