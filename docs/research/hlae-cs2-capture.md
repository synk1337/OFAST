# HLAE's CS2 capture: research notes for OFAST M8

Sources: `advancedfx/advancedfx` at `main` (snapshot taken 2026-10-26, the day of its last push), `advancedfx/injector`, and `advancedfx/advancedfx-prop` (branch `prop`, which holds the CS2 SDK headers). All paths are relative to the repo they come from, and line numbers refer to that snapshot. **Guess:** marks my own inference. No code was built, run or copied.

## 1. How the code gets into CS2

- **Launch.** `hlae/LaunchCs2.cs:48-86` starts `cs2.exe` directly with `-steam -insecure`, plus optional `-w/-h/-sw|-full` and `-afxDisableSteamStorage`. It also sets these environment variables: `SteamAppId=730`, `SteamGameId=730`, `SteamOverlayGameId=730`, `SteamClientLaunch=1`, `SteamPath`, and optionally `USRLOCALCSGO`.
- **Suspended start.** `hlae/Loader.cs:31-49` calls `CreateProcessW` with `CREATE_SUSPENDED|DETACHED_PROCESS|CREATE_NEW_PROCESS_GROUP`. It then spawns `x64\injector.exe` and sends it `{pid, dllPath}` over stdin (lines 59-92). After injection it does `Sleep(2000)` and `ResumeThread` (186-195).
- **Injector.** `injector/Program.cs:90-215` uses `VirtualAllocEx` for the DLL directory and path (RW) and for an image `AfxHook.dat` (RWX). It writes them with `WriteProcessMemory`, calls `FlushInstructionCache` and `CreateRemoteThread` at the image, then polls `WaitForSingleObject` once a second, asking the user whether to keep waiting. Finally it maps the thread's exit code to an error.
  - `AfxHook/AfxHook_x64.asm` is a position-independent stub. It resolves kernel32 functions, temporarily switches the current directory to the DLL's folder, and calls `LoadLibraryExW(LOAD_WITH_ALTERED_SEARCH_PATH)`.
  - The reason, in its header comment (lines 1-10): UCRT forwarders ignore `SetDllDirectoryW`.
  - HLAE ships the injector as a prebuilt zip (`deps/release/injector_bin/CMakeLists.txt`).
- **`-insecure` gate.** `AfxHookSource2/main.cpp:2249` `DllMain`: if the command line lacks `-insecure`, it shows an error box and terminates the process (2257-2267). This is the only active safety guard. A hook that warned on `connect` ("THIS WILL GET YOU VAC BANNED") exists but is commented out (main.cpp:100-150).
- **First hook.** Still in `DllMain` (2272), HLAE patches the IAT of `cs2.exe` itself for `LoadLibraryA/ExA/ExW` (2233-2246). `DllMain` runs on the injector's thread, not the game's main thread (comment at 2277-2281).
- **Module chain.** `LibraryHooksW` (main.cpp:2035-2207) acts on the first load of each module. For tier0.dll it adds IAT hooks for `LoadLibraryEx*`, `GetProcAddress` and `CreateFileW`, and grabs `Msg`/`Warning` (2065-2088). It also handles filesystem_stdio (`AddSearchPath`, vtable slot 31), SDL3/inputsystem (USER32 IAT hooks), engine2, scenesystem, materialsystem2, rendersystemdx11, client.dll (matched by the path ending `csgo\bin\win64\client.dll`), panorama, schemasystem, resourcesystem and particles.
- **Interfaces.** The tier0 `GetProcAddress` hook (1608-1647) wraps `CreateInterface` for client.dll, resourcesystem and filesystem_stdio. On the first client `CreateInterface` call, HLAE gets `Source2Client002` (prop `cs2/Source2Client.h:1`) and swaps these vtable slots (1538-1548):
  - [0] Connect
  - [3] Init
  - [4] Shutdown
  - [11] SetGlobals
  - [35] LevelInitPreEntity
  - [36] FrameStageNotify

  `Connect` (1223-1250) uses the app-system factory to fetch cvar, `Source2EngineToClient001`, `GameUIService_001` and `SceneSystem_002`. `Init` (1337+) registers commands, starts the Rust runtime and reads the schema system.
- **Hooking tools:** Microsoft Detours (submodule; `AfxHookSource2/CMakeLists.txt:109-119`), HLAE's own IAT hooker (`shared/AfxDetours.h:37-260`), vtable slot swaps (`AfxDetourPtr`, `shared/AfxDetours.cpp:157`) and raw byte patches (`MdtMemAccessBegin` plus `memcpy`).

## 2. How it finds CS2's functions and data

- **Pattern scanning.** `shared/binutils.cpp:88` `FindPatternString` scans the module's first PE section (.text) for hex patterns with `??` wildcards. Comments give the string anchor used to find each target in Ghidra. Examples:
  - **engine2 `HostStateRequest::Start`:** `40 53 48 83 ec 40 8b 01 48 8b d9 c6 41 18 01 83 f8 02 74 07 83 f8 04 75 21 eb 0d`, anchored on `"HostStateRequest::Start(HSR_QUIT)\n"` (addresses.cpp:31-66). HLAE uses it to close its streams on quit (main.cpp:152-176).
  - **engine2 `CRenderService::OnClientOutput`:** `4c 8b dc 49 89 5b 10 ... 49 c7 43 b0 ?? ?? 00 00` (addresses.cpp:115). The `??` bytes are a VProf source line number "that changes with engine updates" (line 114).
  - **client.dll `CViewRender::SetUpView` call site:** `48 8b 0d ?? ?? ?? ?? 48 8b 01 ff 90 50 01 00 00 0f 57 ff 84 c0 74 ?? 48 8d 0d` (main.cpp:1073). Here `+0x150` is slot 42, `ISource2EngineToClient::IsPlayingDemo` (prop `cdll_int.h:93`).
  - **Entity list and `GetEntityFromIndex`:** found by decoding RIP-relative operands inside matched code (main.cpp:1160-1180).
  - **Schema system global:** `48 89 05 ?? ?? ?? ?? 4C 8D 0D ?? ?? ?? ?? 33 C0 48 C7 05`, located next to the `"schema_list_bindings"` string (SchemaSystem.cpp:126-141).
- **RTTI vtable finder.** `FindClassVtable(module, ".?AVName@@", ...)` (binutils.cpp:195-310) walks MSVC RTTI from the TypeDescriptor through BaseClassDescriptor, ClassHierarchy and CompleteObjectLocator to the vtable. HLAE uses it for `CViewRender` [4] (builds the projection matrices, main.cpp:1152-1158), `CRenderDeviceDx11` [16] Present (RenderSystemDX11Hooks.cpp:3174-3176), `CRenderContextDx11_SoftwareCommandLists` [32]/[33] (lock/unlock constant buffers), `CMaterialSystem2` [14] FindMaterial, and the scene-object descriptor classes (SceneSystem.cpp:1478-1504).
- **Hardcoded vtable indices:**
  - SceneSystem: `WaitForRenderingToComplete` 26, `FrameUpdate` 74 (addresses.cpp:132,141).
  - Entities: `IsPlayerPawn` 158, `IsPlayerController` 159, render eye origin and angles 173/174. The comments say these were 155/156/170/171 "before 2026-09-23 build" (ClientEntitySystem.cpp:108-160).
  - Software command list `QueueCallback`: 142 (RenderSystemDX11Hooks.cpp:2414).
  - D3D11 context slots 9/21/33/35/36/50/53 (2588-2595). These follow the stable COM ABI.
- **Hardcoded offsets:**
  - **CViewSetup:** width `0x434`, height `0x43C`, fov `0x498`, origin `0x4a0`, angles `0x4b8` (main.cpp:645-650).
  - **Globals:** framecount `+0x4`, frametime `+0x8`, absolute frametime `+0xC`, curtime `+0x30` (MirvTime.cpp:13-40). Interpolation amount is read at byte `+14` (line 57). **Guess:** this is a bug, because every other field uses `index*4`.
  - **CViewRender:** projection at `0x218`, world-to-screen at `0x298` (main.cpp:943,963).
  - **Scene layer:** view pass name at `0x4b8`, flags at `0x48` (SceneSystem.cpp:1680-1682).
- **Schema system.** `SchemaSystem.cpp:10-68` walks `CSchemaSystem` scopes, keeps only `client.dll`, and builds a map from class and field name to offset. `initSchemaSystemOffsets` (88-118) then resolves about 30 fields, for example `C_BaseEntity.m_pGameSceneNode`, `CGameSceneNode.m_vecAbsOrigin`, `CBasePlayerController.m_hPawn`, `C_BasePlayerPawn.m_pObserverServices` and `CPlayer_WeaponServices.m_hActiveWeapon`. The schema covers entity fields only. Engine structs such as CViewSetup and the globals stay hardcoded.
- **Fragility.** Several 2026 CS2 updates broke these hooks:
  - **Commit `b8d5e92` (2026-09-23), "update offsets, functions, and signatures for 14182 build":** touched 8 files. In that build the compiler moved `CViewSetup*` from `r14` to `rbx`, which broke the injected `mov rcx, r14` instruction (see the main.cpp diff in that commit).
  - **2026-01-22, "Fix CViewSetup offsets":** every CViewSetup offset shifted by `-0x40`.
  - **2026-09-26** "Adjust to CS2 v1.41.8.5" and **2026-07-17** "Update Signatures".

  Wildcarded patterns survive small updates. Register choices, struct layouts and vtable slots do not.

## 3. Camera override

- **Where.** HLAE does not detour a function here. It overwrites 16 bytes in place at the `IsPlayingDemo` call inside `CViewRender::SetUpView` (main.cpp:1056-1100). The new bytes are `mov rcx, rbx; mov rax, &ptr; call [rax]; nop`. The engine therefore calls `CS2_Client_CSetupView_Trampoline_IsPlayingDemo(CViewSetup*)` (main.cpp:637-783). That function edits CViewSetup in place and returns the real `IsPlayingDemo()`, so the engine's own `cl_demoviewoverride` logic still runs unchanged.
- **Override order** inside the trampoline:
  1. The game's own view is saved.
  2. Campath: evaluated at `curtime - offset`; "hold" clamps to the path's ends; no extrapolation.
  3. `mirv_camio` import.
  4. `mirv_fov`.
  5. `mirv_input` free cam.
  6. Rust/JS `onCViewRenderSetupView` (728-745).
  7. `.cam` export.
  8. The result is written back and stored as the "last camera".

  A second FOV detour is now disabled (1104-1127).
- **Free camera (`mirv_input`, `shared/MirvInput.cpp`).**
  - **Window messages:** SDL3's IAT entry for `SetWindowLongPtrW` is hooked so HLAE's WndProc sits in front of the game's (main.cpp:378-470, 1839).
  - **Raw input:** `GetRawInputData/Buffer` are hooked (1854, 1878).
  - **Cursor:** centred and hidden from `FrameStageNotify` (1481-1505).
  - **Movement:** `Override(deltaT=absoluteframetime)` integrates per-axis keyboard speeds and mouse deltas, with optional local-space quaternion rotation (MirvInput.cpp:1104-1170).
  - **Pause:** input is ignored while the console is open (`GameUIService::Con_IsVisible`, main.cpp:324).
- **Campath timing.**
  - **Time base:** the client's `curtime` (MirvTime.cpp:13-20).
  - **Demo position:** tick comes from `GetDemoFile()->GetDemoTick()` (prop `cdll_int.h:126` and `:24`). Demo time is computed as `(tick + interp) / 64`, with the tick rate hardcoded (MirvTime.cpp:41-78).
  - **Paused time:** not supported ("we don't support that currently", main.cpp:532); the pause-handling code is commented out (MirvTime.cpp:15-17, main.cpp:1463-1478).
  - **Seeking:** done with `demo_gototick` (`mirv_skip`, main.cpp:1426-1437).
  - **Interpolation:** linear or cubic for doubles, slinear or scubic for quaternions (`shared/CamPath.h:54-66`, `AfxMath.h:416-570`).
  - **Campath drawing:** the projection matrix is captured for drawing the path in-game (main.cpp:916-990).

## 4. Recording

- **Graphics API: D3D11 only.** HLAE hooks rendersystemdx11.dll this way:
  - It hooks the IAT entries for `dxgi!CreateDXGIFactory/1`, uses them to reach `IDXGIFactory::CreateSwapChain` (slot 10), and from there `IDXGISwapChain::Present` (slot 8) (RenderSystemDX11Hooks.cpp:2756-2815).
  - It inline-detours `D3D11CreateDevice`, then `CreateRenderTargetView` and the immediate-context methods (2567-2671). The comment explains why: "We can not use vtable detours here, because something writes them back" (2571).
  - It also hooks the engine's `CRenderDeviceDx11::Present` (slot 16).

  There is no Vulkan or D3D12 code. **Guess:** a `-vulkan` launch would break capture.
- **Multiple passes per frame.** `CRenderService::OnClientOutput` is detoured (RenderServiceHooks.cpp:50-88):
  - The main pass renders normally.
  - For each group of extra streams, HLAE waits for rendering to finish, calls `SceneSystem::FrameUpdate(1)` and runs `OnClientOutput` again. The same game frame is re-rendered once per pass group, each time with different draw filters.
  - The swapchain Present of the extra passes is suppressed (3546-3567, 4182-4199, 2721-2744).
- **Hook points inside a frame.** HLAE detours scenesystem's `CreateRenderContextPtr` and matches the debug format strings it receives:
  - `"#%s/%s/LayerClear"` with `Player 0` and `Clear Color Depth Stencil` / `ClearSmokeTargets`
  - `"#%s/SetupLightsAndViewConstants"` with `CSGOHud`

  On a match it queues a callback object into the software command list via vtable slot 142 (2974-3060). This gives render-thread hooks for before-clear, smoke, before-HUD and after-post-processing. Commands flow from the engine thread to the render thread through queues in `RenderCommands.h`.
- **Pass types:**
  - **Capture types:** Rgb, Rgba, DepthRgb, DepthF. **Capture points:** BeforePresent or BeforeUi (StreamSettings.h ≈100-115).
  - **Presets:** normal, hudBlack, hudWhite, depth, world, playersMatte, weaponsMatte, viewModelMatte, particlesBlack, particlesWhite (RenderSystemDX11Hooks.cpp:4288-4470).
  - **Per-group actions:** each of viewModel, particles, firstPersonLegs, shells, weapons, players, world, sky, smoke and overlays can be set to draw, noDraw or zOnly (SceneSystem.cpp:319-435).
  - **How actions apply:** detours of scenesystem `InitDrawingData`/`DrawSceneData` (≈1665-1700) classify each scene object by its descriptor-class vtable (RTTI), material name and layer context (706-717).
  - **Per-stream console commands:** a stream can run commands before and after its pass, for example `r_csgo_render_overlays 0`.
  - **Mattes:** a matte is an RGBA capture with a cleared background where everything except the target is zOnly (it still occludes but adds no colour), so alpha carries the mask (`playersMatte`, 4319-4353).
- **Depth.** `CDepthCompositor` (1075-1745) finds the depth-stencil view through the `ClearDepthStencilView`/`OMSetRenderTargets` hooks. It recovers zNear/zFar from the projection matrix by LU decomposition (≈1510-1550). Then HLAE's own `afx_depth_ps_5_0` shader writes R32F or 24-bit RGB depth, in inverse, linear, logE or pyramidal mode. Smoke depth can optionally be composited in.
- **GPU readback.** `CAfxCapture` (264-860) copies each frame into a `D3D11_USAGE_STAGING` texture, using `ResolveSubresource` first for MSAA or a GPU scale pass when needed. It maps that texture one frame later to avoid stalling (380-398). With 3 staging textures in flight it blocks, which rate-limits the game (355-360). A worker thread converts RGBA to BGR/BGRA, or float depth to an image, using a thread pool, and sends the result to the output stream (790-857).
- **Output (`shared/AfxOutStreams.cpp`).**
  - **Image sequences:** TGA or BMP, and EXR for float depth (20-50).
  - **FFmpeg:** HLAE runs `ffmpeg.exe` (path from `ffmpeg.ini` or the HLAE folder, 209-230) with stdin/stdout/stderr connected to named pipes `\\.\pipe\AfxHookSource_FFMPEG_In_<pid>_<ptr>`. Raw frames go in with overlapped `WriteFile` (290-575).
  - **Command template:** `{FFMPEG_PATH} -f rawvideo -pixel_format {PIXEL_FORMAT} -framerate {FRAMERATE} -video_size {W}x{H} -i pipe:0 -vf setsar=sar=1/1 <options>` (RecordingSettings.cpp:394). Pixel formats are bgr24, bgra, gray and grayf32le (AfxOutStreams.cpp:232-251).
  - **Presets:** x264 crf 22, lossless libx264rgb, huffyuv, raw, ProRes 4444 with or without alpha (RecordingSettings.cpp:24-59), and `afxSampler30`, which blends frames down for motion blur (line 64).
- **Deterministic timing.**
  - **cvars:** `RecordStart` (5260-5398) sets `host_framerate` when an fps override is given, `engine_no_focus_sleep 0`, `r_always_render_all_windows 1` and `r_wait_on_present 1`, and restores them at the end (5401-5466).
  - **Clock:** `Plat_FloatTime` IAT hooks in scenesystem.dll and panorama.dll advance their clocks by the game's frametime while recording (MirvFix.cpp:13-82).
  - **Animations:** `cl_interp_ratio` is forced from 0 to 1 during demo playback to stop animation stutter (main.cpp:1511-1520, commit `f77de71`, 2026-08-14).
  - **Smoke:** full-resolution smoke is forced when needed (3376-3410, 3514-3540).
  - **Audio:** recorded with CS2's own `startmovie <key> wav`. A `CreateFileW` hook redirects that file to `<take>\audio.wav` (main.cpp:1688-1712).
  - **Take folder:** each recording gets a `takeNNNN` folder with one subfolder per stream, plus `campath.xml` and `cam_main.cam`.

## 5. Live data and bone capture

- HLAE has **no bone capture for CS2.** `mirv_agr`/AfxGameRecord exists only in the CS:GO hook (a code search hits only `AfxHookSource/ClientTools.cpp`), and `AfxHookSource2/CMakeLists.txt` does not compile `AfxGameRecord.cpp`.
- What scripts can read (ClientEntitySystem.cpp:529-730): the entity list, class names, handles, health, team, origin (the scene node's absolute origin), render eye origin and angles (vtable calls), active weapon, observer mode and target, player name and SteamID, and named attachments (pattern-found `LookupAttachment`/`GetAttachment`, 416-422). There are also add/remove-entity and game-event callbacks, and a websocket client `mirv.connect_async` (lib.rs:2334-2350), which is HLAE's live link to outside tools.
- **Guess:** for OFAST, bones would come from `CSkeletonInstance`/model state, located through the schema. SceneSystem.cpp:674 mentions `NetworkVar_m_modelState@CSkeletonInstance`. There is no prior art for this in CS2.

## 6. Rust parts already in HLAE

- `lib/afx_hook_source2` is a `staticlib` (Rust 2024 edition) linked into the C++ DLL through corrosion (`AfxHookSource2/CMakeLists.txt` ≈102). Its dependencies are boa_engine (JavaScript), async-tungstenite and async-net (websockets), and futures.
- `src/lib.rs` (148 KB) is the mirv-script runtime. Its global `mirv` object (3211-3375) offers `message`, `warning`, `exec`, `load`, `run_jobs` and `connect_async`; the hooks `onRecordStart/End`, `onGameEvent`, `onCViewRenderSetupView` (a camera override), `onClientFrameStageNotify` and `onAddEntity/RemoveEntity`; and `getEntityFromIndex`, `getMainCampath`, `getCurTime`, `getDemoTick`, `isPlayingDemo`. JS modules: `campath`, `cvar`, `events`, `fs`, `math` (`src/advancedfx/js/*`).
- The script job queue is pumped from `FrameStageNotify` (main.cpp:1460, 1534). Console commands `mirv_script_exec` and `mirv_script_load` live in `AfxHookSource2Rs.cpp:298-320`.
- `advancedfx/campath/mod.rs` is only an FFI wrapper over the C++ CamPath (`shared/CamPathRs.cpp`); interpolation stays in C++.
- So in HLAE, all hooking, rendering and capture are C++. Rust handles only scripting and networking.

## 7. Hardest parts and risks for a clean-room Rust remake

1. **The camera hook site** depends on register allocation and hardcoded CViewSetup offsets, and both broke in 2026. Find `SetUpView` from a string or cvar anchor (`cl_demoviewoverride`), use a proper detour, and check offsets at runtime (width and height must match the swapchain, fov must be in range). If any check fails, refuse to hook.
2. **Re-rendering one frame several times** (re-calling `OnClientOutput` plus `FrameUpdate`) is the most fragile area. HLAE committed fixes for crosshair, HUD, flicker and scope issues from Aug to Oct 2026. **Guess:** start with a single pass (color and depth). For mattes, first try replaying the demo once per pass, which relies on deterministic playback and needs verifying.
3. **Scene-object classification** relies on RTTI vtables and material-name heuristics, and the insertion points depend on render-layer name strings (`"Player 0"`, `"CSGOHud"`). Both change across patches.
4. **Threading.** Render-thread work goes through command-list vtable slot 142. HLAE notes that D3D11 vtable hooks get "written back", so OFAST needs inline detours rather than vtable swaps.
5. **Timing.** OFAST needs `host_framerate`, the `Plat_FloatTime` fixes and the `cl_interp_ratio` fix. Paused time is unsupported in HLAE, and the tick interval is hardcoded to 64.
6. **Building blocks HLAE has no Rust version of:** inline detours, IAT hooks, vtable swaps, a PE pattern scanner, an MSVC RTTI walker and a schema walker. Every `extern` hook needs `catch_unwind`, and `DllMain` must do no work under the loader lock. **Guess:** the `retour`/`minhook` and `windows` crates would help (not from HLAE). The single x64 calling convention keeps hook signatures simple.
7. **Policy.** Keep the `-insecure` gate. Also consider refusing to load outside demo playback and blocking `connect`; HLAE's guard for that is commented out.
8. **D3D11 only.** Bones need new reverse-engineering work, with no prior art.

**Suggested first spike, "ofast-capture hello":**
- A Rust launcher that mirrors `LaunchCs2.cs`: start `cs2.exe` suspended with `-steam -insecure` and the Steam environment variables, then `CreateRemoteThread(LoadLibraryW)` a Rust `cdylib`.
- The DLL:
  - checks for `-insecure`
  - IAT-hooks `LoadLibraryExW` in `cs2.exe`
  - wraps client.dll's `CreateInterface` to get `Source2Client002` and `Source2EngineToClient001`
  - swaps `SetGlobals` (slot 11) and `FrameStageNotify` (slot 36), and prints curtime and demo tick each frame through tier0 `Msg`
  - walks the schema system and dumps the client.dll offsets to JSON, so they can be compared with OFAST's demo parser
- **Done when:** the log appears during an `-insecure` demo playback, and CS2 quits cleanly.
- **Spike 2:** hook Present, read back through a staging texture, and pipe raw BGRA to `ffmpeg -f rawvideo -i pipe:0` at `host_framerate 60`.
- **Spike 3:** override the camera.
