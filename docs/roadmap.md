# Roadmap

Ordered so that **Demo → Unreal scene** works first. Every milestone ends in something usable on its own.

| # | Milestone | Done when |
|---|---|---|
| **M0** | Foundations + spikes | Workspace, CI, oracle harness. Spikes: VPK + KV3 read · demo header + player positions for one round · one model → glTF → UE 5.8. |
| M1 | Asset core | Textures, materials, meshes and a full map export to glTF + material/light manifests. |
| M2 | UE plugin + one-click map import | A CS2 map opens in UE with correct scale, master materials, lights from entities and a Lumen preset. |
| M3 | Demo → Sequence blockout | Full demo parser; proxy players move, POV camera, kill markers. Studio demo browser. |
| M4 | Real players | Skeletons + animation clips, agent models + weapons, replay rig, deaths. |
| M5 | First-person POV | Arms + viewmodel, event-driven animations, viewmodel FOV. |
| M6 | Grenades & FX | Trajectories; smokes / molotovs / flashes / HE / tracers / impacts in Niagara. |
| M7 | Camera tools | Campath editor, smoothing, cinematic presets; HLAE `.cam` import. |
| M8 | ofast-capture | Free cam, campaths, multi-pass streams + FFmpeg; then live bone capture. |
| M9 | Render presets | Movie Render Graph, depth/matte passes for compositing. |
| M10 | Skins (stretch) | Default finishes, then paint kits / wear / seed / stickers. |

## M0 progress

- [x] Repository, license, docs skeleton, decision log
- [x] CI workflow
- [x] VPK reader (synthetic tests in CI, oracle tests on the local CS2 install — MD5 + BLAKE3 chunks, CRCs)
- [x] Binary KV3 decoder v1–v5, none/LZ4/ZSTD — all 245,863 KV3 blocks in CS2 decode exactly
- [x] KV3 text writer
- [ ] KV3 text parser (needed to diff against other tools' decompiled output)
- [x] Resource container reader
- [ ] `ofast` CLI (`vpk list/extract/verify`, `kv3 dump`)
- [ ] Demo header + player positions for one round
- [ ] One model → glTF → UE 5.8

## Hard problems we know about

- **Player animation.** Demos contain no skeleton poses; CS2 computes them while playing the demo. Plan: a "replay
  rig" in UE driven by demo data (M4), then exact live capture through ofast-capture (M8).
- **CS2 updates.** Each patch can change demo protobufs and game internals. The capture DLL reads field offsets
  from CS2's own schema system at runtime to limit breakage.
- **Skins and particles** have no open-source reference implementation; they are late milestones.
