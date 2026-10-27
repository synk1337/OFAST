# Cameras (`.cameras.json`) and HLAE's camera files

M7, [decision 021](../decisions/021-cameras.md). Every camera of a round is made in Rust (`crates/ofast-core/src/camera`,
`export::cameras`) and written to the round's cameras file, next to its replay file; Unreal only keys them
(`tools/unreal/replay_cameras.py`, used by `import_demo.py` and `import_cameras.py`).

```bash
ofast export-demo match.dem --round 17 --out exports --players \
    --shots 153245:orbit,153359:tripod,154410:victim --smooth 0.5 --hlae my_path.xml
powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_demo.py -Replay exports/match_r17.ofast-replay.json
# later, other shots for the same round: export again, then only the cameras (seconds)
powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_cameras.py -Cameras exports/match_r17.cameras.json
ofast camera-to-hlae exports/match_r17.cameras.json --camera 0 --out orbit.xml   # back to HLAE
```

## The cameras file (version 1)

Everything in CS2's units and angles, as the replay file: Source units, `(pitch, yaw, roll)` degrees (pitch positive
looking down, yaw counter-clockwise from +x), a field of view as CS2 counts it (horizontal at 4:3: 90 is CS2's
default, 106.26° on a 16:9 picture).

| Key | What |
|---|---|
| `format`, `version` | `"ofast-cameras"`, 1 |
| `demo`, `map`, `round`, `start_tick`, `end_tick`, `tick_interval` | as the replay file's |
| `server_tick_offset` | how far the server's tick runs ahead of the demo's (`RoundTrack::server_tick_offset`; `null` without one): HLAE's times are `(tick + offset) × tick_interval` |
| `plan` | what was asked: `shots` (kill tick → `pov`, `victim`, `behind`, `orbit`, `tripod`, `above`), `smoothing` (0 – 1), `hlae` (the files read) |
| `players` | controller (as text) → the player's last known name (Unreal finds their camera by it: `POV <name>`) |
| `eyes` | controller (as text) → `[tick, x, y, z, pitch, yaw]` on every sample tick with a pawn: the first-person camera |
| `cameras` | the cameras of their own: `name` (`Orbit around XANTARES, kill at 153245`), `kind` (`behind`, `orbit`, `tripod`, `above`, `hlae`), `subject` (controller or `null`), `keys`: `[tick, x, y, z, pitch, yaw, roll, fov]` (ticks may fall between ticks: an HLAE recording's frames) |
| `cuts` | in order: `{start, end, eyes: controller}` or `{start, end, camera: index into cameras}`, ticks, `end` not included |

### Eyes

The first-person camera: the eyes (feet + `m_vecViewOffset`), looking along the eye angles turned by recoil
(`export::punch`: 0.9 × the aim punch + the view punch; [demo.md](demo.md#recoil-on-the-view)) — what CS2 shows when
spectating that player in first person. Without CS2's content (`export-demo` without `--players`) there is no recoil.

**Smoothing** (`--smooth`, the Studio's "First-person smoothing"): the eye angles smoothed before the recoil is added,
by a Whittaker smoother (`camera::smooth`): the curve closest to the angles whose second differences stay small,
`Σ wᵢ (zᵢ − yᵢ)² + λ Σ (Δ²zᵢ)²`, solved as one pentadiagonal system. λ = 10^(0.5 + 3.5 s) for a strength s: 0.5 gives
λ ≈ 178 (a wiggle shorter than about 0.36 s goes), 1 gives 10 000 (about a second). The view is **pinned** (weight
10⁹) on the tick before, of and after each of the player's shots and kills: the bullets, tracers and hits come from
the original aim, so the crosshair must be on it there. Yaw is unwrapped first, so a turn across ±180° stays a turn.

### Cuts

Each kill's cut runs from the cut before it to half a second after the kill (`CUT_AFTER_KILL_S`; the last kill's to
the round's end), as `import_demo.py` always cut a round. Its shot (`plan.shots[kill tick]`, the killer's eyes by
default):

| Shot | The cut |
|---|---|
| `pov` | the killer's eyes |
| `victim` | the killer's eyes, then the victim's for the last 4 s up to the kill's tick, then the killer's again |
| `behind`, `orbit`, `tripod`, `above` | the killer's eyes, then the shot's camera for the last 4 s (`LEAD_S`) to the cut's end |

An HLAE camera takes the cuts over for its whole time in the round (those it covers are cut short or split).

### Shot cameras

`camera::shots`, from the round's poses (feet, eyes, view angles, alive) on every sample tick of the shot. Moves are
smoothed with the same Whittaker smoother (positions, look angles), so a camera follows a player's movement, not their
mouse. Walls: the map's collision, the shapes that stop a body (`world_physics`, glass and grates included): a camera
whose place lies past a wall (or within 6 units of one, `WALL_MARGIN`) is pulled in front of it at once, and eases
back out at 60 units a second, so a wall passing behind a player does not make it jump in and out.

| Kind | Where | Looks at | FOV |
|---|---|---|---|
| `behind` | 64 units behind the killer's eyes along his (smoothed) view, 16 to his right, 6 up | where he looks, 400 units out | 80 |
| `orbit` | 100 units from his chest, 22 above it, turning 12° a second; at the kill behind him relative to the victim, or the clearest of 16 starts (sides, front, both directions) | his chest | 70 |
| `tripod` | a fixed point that sees both the killer's eyes and the victim's head at the kill (a headshot over cover shows no more of him), within 75° of each other: 120 candidates — 36 across the line between them (both sides, 0.7 – 1.4 × their distance away, at least 150 units, 24 or 72 units above the higher one), and 42 around each of them, turned 90 – 180° from the line to the other, 80 – 220 units away (behind the killer looking past him at the victim, behind the victim looking back at the killer: what a narrow corridor leaves) —, scored by how often it sees them in the last 2 s, nearer better | between the two (0.65 killer, 0.35 victim), smoothed | the angle between them + 30, 45 – 85 |
| `above` | 300 units above his feet, 90 behind relative to the victim | in front of his feet | 75 |

A tripod no point of which sees both (a kill through a wall) becomes a `behind` camera, with a warning.

## HLAE's files

Read and written by `camera::hlae`, from HLAE's source (advancedfx, MIT): `shared/CamPath.cpp`, `shared/CamIO.cpp`,
`shared/AfxMath.cpp`, `AfxHookSource2/main.cpp`.

**Time.** HLAE's times are CS2's game clock, `curtime` (`CMirvTime::curtime_get`: the client's globals), which counts
the server's ticks: `curtime = (demo tick + server_tick_offset) × tick_interval`. The offset is constant within a demo
(2403 ticks on the local match demo). Checked in CS2 with ofast-capture (2026-10-27, apps/capture): playing the
Aurora vs FURIA de_dust2 demo, `curtime × 64 − demo tick` was 134067 plus a fraction, the server tick offset OFAST
reads from that demo, and after a seek up to 2 ticks more (the demo tick moves in whole steps, the clock smoothly,
and CS2 draws a little behind its clock): an HLAE time and an OFAST tick agree within 2 ticks (30 ms).

The cameras of a cameras file also play inside CS2 itself, through ofast-capture (`camera <file> <number>`,
`camera <file> eyes <controller>`): a player's eyes from the file and CS2's own first-person view of that player
frame the same picture to a few pixels.

**Campath** (`mirv_campath save`): an XML `<campath>` with optional attributes `positionInterp` and `fovInterp`
(`default`, `linear`, `cubic`), `rotationInterp` (`default`, `sLinear`, `sCubic`), `offset` (seconds, added to the
points' times) and `hold`, and `<points>` of `<p t x y z fov rx ry rz qw qx qy qz selected/>`: the time, the position,
the field of view as CS2 counts it, roll (`rx`), pitch (`ry`) and yaw (`rz`) in degrees, the same rotation as a
quaternion (HLAE reads the quaternion when all four are there, else the angles; the quaternion is
`yaw (z) · pitch (y) · roll (x)`, right-handed, Source's axes). HLAE plays a path between its first and last points
only (clamped there with `hold`).

**Interpolation** (`default` is cubic): position and field of view by a cubic spline through the points
(Numerical Recipes' `spline`/`splint`, called with a slope of 0 at both ends: the path starts and ends at rest) or
linearly; rotation by James McEnnan's quaternion spline (`qspline`, CC0: angular rates continuous at the points, solved iteratively — HLAE runs at most two
extra passes, tolerance 10⁻⁶ —, each interval a third-order slew from one point to the next) or spherical linear.
HLAE needs 4 points for a cubic path and plays nothing with fewer; OFAST falls back to linear there. OFAST samples the
path on every whole tick between its ends (and at both ends), so the camera moves as HLAE moves it.

**`.cam`** (`mirv_camio export`): `advancedfx Cam`, `version 1|2`, an optional `scaleFov none|alienSwarm`, a
`channels` line, `DATA`, then one line per frame, `time x y z xRotation yRotation zRotation fov` with x the roll,
y the pitch, z the yaw. CS2's HLAE writes `curtime` and the field of view scaled to the picture
(`AlienSwarm_FovScaling`: `2·atan(tan(fov/2) · aspect / (4/3))`, the picture's own horizontal field of view) without
a `scaleFov` line; the file does not say the picture's aspect, so OFAST takes 16:9 (`HLAE_ASPECT`). `scaleFov none`
stores CS2's own. OFAST writes version 2, `curtime` times and the field of view at 16:9; HLAE's import plays a file
from when it is started (its first frame then), so the times only need to be right relative to each other there.

## In Unreal

`replay_cameras.py`:

- **Eyes**: each player's `POV <name>` camera (a CameraActor, 106.26° at 16:9) keyed linearly on `eyes`, after
  dropping keys a straight line gives (0.2 cm, 0.1°).
- **Shot and HLAE cameras**: a CineCameraActor each (label = `name`, tags `OFAST_replay`, `OFAST_camera`), keyed on
  every key with cubic keys (auto tangents: smooth between ticks in slow motion; sub-frame keys for HLAE frames), its
  focal length keyed from the field of view on a 16:9 sensor (36 × 20.25 mm: `focal = 18 / tan(hfov / 2)`), depth of
  field off (focus method Disable).
- **Cuts**: the camera cut track.
- **Who sees what**: each `POV <name>` camera owns what that player's own view must not show — their body (and a
  proxy's visor), their muzzle flashes and tracers in the world, the blood of hits on them, their cases in flight in
  the world — whose primitives are `bOwnerNoSee`, and what only their view shows — their viewmodel, its flashes, cases
  and their own tracers — which are `bOnlyOwnerSee` (`import_demo.apply_owners`). Unreal checks them against the view
  target, the camera a cut uses, so any camera, including one the user adds, shows the round right. Visibility keys
  only follow life and death. A case thrown into the world is two actors: its flight, hidden from its shooter's eyes
  (their view shows the first-person case), and the case lying after it, seen by everyone.
- **Muzzle lights**: a light cannot be hidden from one camera only, so a shooter's third-person muzzle light is keyed
  visible outside the cuts through their eyes and the one on their viewmodel inside them (`light_changes`); tagged
  `OFAST_light_tp_<controller>` / `OFAST_light_fp_<controller>` and keyed again by `import_cameras.py`.

`import_cameras.py` changes a round's cameras without importing it again: the eyes keyed again, the `OFAST_camera`
cameras replaced, the cut track replaced (cuts made by hand too; cameras made by hand stay), the muzzle lights (found
by their tags) keyed again. About a minute, most of it Unreal starting. It refuses a round built before M7 (no
`OFAST_start_tick` on its sequence): that one hides its players by keys that follow its old cuts.

`import_demo.py` stores the demo's clock on the sequence (asset metadata `OFAST_start_tick`, `OFAST_tick_interval`,
`OFAST_server_tick_offset`, `OFAST_demo`, `OFAST_map`, `OFAST_round`), for the scripts that work on a round already
built: `import_cameras.py`, `shoot_sequence.py` (screenshots through whichever camera the cuts use at given ticks) and
`export_camera.py`.

### Back to HLAE

`export_camera.py -Sequence <sequence> -Name <camera's label> -Cameras <file>` writes a camera of the round (one
made in Unreal, or any other) to a cameras file as Sequencer plays it: the sequence set to every frame from the
camera's first key to its last, the camera's place, rotation and horizontal field of view read in the world (an
attached camera, or one aimed by look-at tracking, comes out as it shows), turned into CS2's units, angles and field
of view. `ofast camera-to-hlae <file> --out path.xml` (or `.cam`) then writes it for HLAE: a campath of every tick
at its game time, default interpolation; a `.cam` with the field of view at 16:9.

## Checked

- `cargo test -p ofast-core camera`: angles and quaternions both ways, fields of view, the smoother (a line stays a
  line, jitter goes, pins hold, yaw across ±180°), the shots (behind, orbit, tripod, above in the open; walls pull in
  and ease out), campaths as HLAE saves them (sampled linearly halfway: halfway along and turned 45°), cubic paths
  through their points without jumps, `.cam` round trips (roll / pitch / yaw columns, field of view scaling), the cuts
  of a plan, an HLAE camera taking cuts over, smoothing pinned at a shot.
- Owner visibility on de_dust2 round 17 (2026-10-26): through XANTARES's eyes the view is clear; with
  `bOwnerNoSee` off his head fills it; from behind him his body shows.

Not yet: a campath or `.cam` made in CS2 with HLAE (HLAE is not installed here), and Movie Render Queue renders with
owner visibility (M9).
