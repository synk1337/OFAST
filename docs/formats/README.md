# CS2 file formats

One page per format OFAST reads. Each page explains the byte layout in plain terms, lists what is still unknown,
and links the sources the knowledge came from. The code in `crates/ofast-core` follows these pages, and the
synthetic test fixtures are built straight from them.

| Format | Files | Page | Status |
|---|---|---|---|
| VPK archive | `pak01_dir.vpk`, `pak01_000.vpk`… | [vpk.md](vpk.md) | in progress (M0) |
| KeyValues3 | text `.vmat`, `.vpcf`…; binary inside compiled resources | [kv3.md](kv3.md) | in progress (M0) |
| Resource container | every `*_c` file (`.vmat_c`, `.vmdl_c`, `.vtex_c`…) | [resource.md](resource.md) | in progress (M0) |
| Demo | `.dem` | demo.md | planned (M0/M3) |
