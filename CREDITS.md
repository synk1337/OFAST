# Credits and references

OFAST is written from scratch, but it stands on years of public reverse-engineering work by the Source modding
community. These projects are studied for knowledge and used as **test oracles** (we compare our output to theirs).
Where OFAST code is closely derived from one of them, the file says so in its header and the original license notice
is kept below.

| Project | What we learn from it | License |
|---|---|---|
| [ValveResourceFormat / Source 2 Viewer](https://github.com/ValveResourceFormat/ValveResourceFormat) | Source 2 resource container, KeyValues3, textures, meshes, materials, maps, animations | MIT |
| [ValvePak](https://github.com/ValveResourceFormat/ValvePak) | VPK archive format | MIT |
| [demoparser2](https://github.com/LaihoE/demoparser) | CS2 demo format and entity decoding | MIT |
| [demoinfocs-golang](https://github.com/markus-wa/demoinfocs-golang) | CS2 demo format | MIT |
| [HLAE / advancedfx](https://github.com/advancedfx/advancedfx) | Game capture, camera paths, `.cam` format, recording streams | MIT |
| [SourceIO](https://github.com/REDxEYE/SourceIO) | CS2 shader parameter meanings | MIT |
| [Esoterica](https://github.com/BobbyAnguelov/Esoterica) | The task serializer whose encoding CS2's AnimGraph2 pose recipe follows (`demo::recipe`) | MIT |
| [MurmurHash2](https://github.com/aappleby/smhasher) | `MurmurHash64B`, the hash Source 2 names resources by (`resource::resource_id`, written from the reference) | public domain, Austin Appleby |
| [meshoptimizer](https://github.com/zeux/meshoptimizer) | Vertex/index codec bitstreams (OFAST has its own decoder); its test vectors are copied into `src/meshopt.rs` tests | MIT, © Arseny Kapoulkine |
| [Valve Developer Community wiki](https://developer.valvesoftware.com/) | File formats, units, coordinates | CC BY-NC-SA 3.0 (docs only) |
| [SteamDatabase GameTracking-CS2](https://github.com/SteamDatabase/GameTracking-CS2) | Protobuf definitions (Valve's demo `.proto` files are copied unchanged into `crates/ofast-core/proto/`), game file listings | see repository |

Not used as code sources (incompatible or missing license): UEditingTools (GPL-3.0), afxlivelink (no license).
