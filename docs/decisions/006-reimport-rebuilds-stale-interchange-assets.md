# 006 — A map's re-import rebuilds the Interchange assets that no longer match its glTF

- **Date:** 2026-10-16
- **Status:** accepted

## Context

`import_map.py` imports a map's glTF with Interchange's scene import into `/Game/OFAST/<map>/<map>` (meshes,
material instances, textures, the import record `SceneImport_<map>`), with `replace_existing`. Running it again after
a re-export did not update the materials: `tools/unreal/probe_materials.py`, which compares every Interchange
material instance with what the current glTF says (texture parameters by image name, colour factors), found

| map | stale instances | textures no material uses any more |
|---|---|---|
| de_mirage | 366 of 402 (decals and overlays with normal maps of an older export) | 447 of 1166 |
| de_dust2 | 1 of 718 (a `csgo_environment` from before the colour bakes) | 2 of 738 |
| de_nuke | 3 of 602 (same) | 4 of 892 |
| de_anubis | 0 of 386 | 0 of 737 |
| de_inferno | 739 of 922 (528 with a tint factor the glTF no longer has, 721 with an old roughness texture) | 2087 of 3186 (most of its 12 GB) |

Decision 005 found the symptom on de_inferno's environment materials and moved them to an OFAST master, leaving the
cause. Read in UE 5.8's sources, it is three Interchange behaviours together:

1. **Importing a scene where its import record already exists is turned into a re-import** of that record
   (`UInterchangeManager`: `replace_existing` converts it).
2. **A re-import changes nothing on existing material instances** by default: `UInterchangeGenericAssetsPipeline`'s
   `ReimportStrategy` is `ApplyNoProperties`, and `SetupReimportedMaterialInstance` returns at once.
   `ApplyPipelineProperties` would not be enough either: the glTF translator only writes parameters that differ from
   their defaults (`SetColor(BaseColorFactor, …, white)`), and the re-import only writes the parameters the new
   material has, so a tint factor or texture the glTF dropped stays.
3. **A re-import never recreates an asset deleted since the import before**: the record remembers it, and
   `FFactoryCommon::CanProceedWithFactoryNode` skips it as "deleted or renamed by the user".

## Decision

Before every scene import, `import_map.py` (`evict_stale_interchange_assets`) deletes what the current glTF would not
give, and the import record, so that Interchange's import is always a first import:

- **Material instances made from another version of the glTF file**, or for a material the glTF no longer has. Each
  instance's import data records the glTF's MD5 (the asset registry's `AssetImportData` tag, read without loading the
  asset), so this needs no knowledge of how Interchange converts a glTF material.
- **Textures for images the glTF no longer has, or whose PNG changed** since they were imported (recorded timestamp,
  then MD5): the exporter's names are not all content hashes (a CS2 texture keeps its file name, `_2` when two
  differ), so a re-export can give an existing name new pixels. Material instances that use a deleted texture go
  too.
- **`SceneImport_<map>`.** A first import reuses the materials and textures still there (by name, Interchange's
  "reuse existing materials" and its rule not to overwrite existing textures from a scene) and overwrites the meshes.

The deletions run before the level is opened: with its objects in memory, Unreal's force delete costs far more per
asset (de_nuke: 602 instances in 272 s with the level open; de_inferno: 922 instances and 2824 textures in 190 s
before it).

The same holds for OFAST's own assets (`/Game/OFAST/<map>/Blend` and `Skybox`): `import_texture` imports a PNG again
in place when it changed since (same timestamp-then-MD5 test), and the instances and textures a run does not use are
deleted (de_inferno: 1359 of 2810 files; the folder went from 11 GB to 2.3 GB, and its Interchange textures from
12 GB to 2.6 GB).

Re-running without a re-export deletes nothing but the record: the check takes 1–4 s per map (de_inferno 3 s), and
de_mirage's scene import 9–11 s (17 s when 402 instances were rebuilt); whole runs of the five maps took 80–230 s.
After a re-export, every Interchange material of that map is rebuilt once.

Checked on the five imported maps: `probe_materials.py` finds no stale instance and no unused texture any more (see
Context for before). Renders against the previous ones (`test-data/shots/fresh_materials/`, luminance relative to
CS2's screenshots): de_mirage's decals and overlays changed, its four views by 0.4–6 %; de_inferno's views by under
0.5 % (its visible surfaces already used OFAST's instances); de_dust2's by a few sRGB levels at most. de_nuke and
de_anubis changed for other reasons: their exports are older than decision 005's format, so this importer gives
de_nuke's two height blends Interchange's stand-in (the vertex paint shows as red asphalt), and de_anubis renders with
the Hable fit this branch has (1.37 / 1.32 of CS2; the Hable session's newer rule gives 1.02 / 1.00).

## Alternatives considered

- **`ReimportStrategy = ApplyPipelineProperties` through an override pipeline stack.** Leaves removed parameters
  (see 2); clearing every instance's parameters before the re-import would fix that, but an instance whose factory
  node Interchange disables (its import data names another file) would then stay cleared.
- **Delete the whole Materials folder every run.** Simple and correct, but rewrites ~900 packages on every run, and
  without also deleting the record the deleted instances would not come back (see 3).
- **Compare each instance with the glTF material (what the probe does) and delete only those that differ.** Exact
  per material, but mirrors Interchange's glTF conversion (parameter names, default values, texture naming), which
  OFAST does not own; a missed parameter means a stale material again. Rebuilding everything after a re-export costs
  seconds to a few minutes, once.
- **Import into a new folder per export.** Never stale, but leaves every older copy (meshes included) behind.

## Consequences

- An Unreal render of a map shows what its current export says; earlier renders (and calibrations) were made with
  partly stale materials. The probe (`probe_materials.py`) checks it.
- The first import after a re-export takes longer (de_inferno: 190 s of deletions, once). The test compares file
  bytes: a re-export that only re-encodes a PNG also re-imports it.
- Every run hashes the PNGs whose recorded timestamp differs (the exporter rewrites them all; a texture Interchange
  kept keeps its old timestamp): at most de_inferno's 2.6 GB, about 6 s.
- A GPU crash (device removed) stopped one of ten runs during its warm-up frames (de_anubis, nothing deleted or
  rebuilt in that run); the same run again went through. Known before (roadmap: "GPU crash on re-import").
- Everything in `/Game/OFAST/<map>/<map>` belongs to the import: edits made there by hand are lost when the glTF
  changes.
