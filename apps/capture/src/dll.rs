//! The DLL's side, inside CS2 (decision 022). `DllMain` checks `-insecure` first: without it the DLL refuses to load
//! (Windows unloads it at once, nothing done). With it, a thread finds CS2's interfaces, checks every slot it needs
//! ([`crate::cs2`]), and only then swaps the client's `FrameStageNotify` for OFAST's, which runs on the game's own
//! thread each frame.
//!
//! Each frame stage seen is logged once, and while a demo plays, every 32nd rendered frame logs the demo's tick and
//! the game's clock (`curtime`). Their distance, `curtime × 64 − demo tick`, is the server tick offset OFAST reads from
//! the demo (`RoundTrack::server_tick_offset`), the base of HLAE's camera file times (`camera::hlae::Clock`): the log
//! checks it. The demo tick is read on the game's thread, at the rendered frame's stage, and kept for the other
//! threads ([`demo_tick`]): the render thread never calls the engine.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

use crate::cs2::{self, Interface, Module};
use crate::{gate, hook, log};

/// The frame stage of a frame rendered for display (`FRAME_RENDER_PASS`, HLAE's headers).
const RENDER_STAGE: i32 = 12;

/// Every how many rendered frames the clock is logged during a demo.
const LOG_EVERY: u64 = 32;

/// How long the DLL waits for `client.dll` and `engine2.dll` (CS2 still starting).
const WAIT_FOR_MODULES: Duration = Duration::from_secs(120);

/// CS2's tick rate (HLAE's too: demos are 64-tick).
const TICK_RATE: f64 = 64.0;

/// [`DEMO_TICK`] while no demo plays.
const NO_DEMO: i64 = i64::MIN;

struct Game {
    engine: Interface,
    engine_module: Module,
    /// Where the client keeps the globals pointer.
    globals: usize,
}

static GAME: OnceLock<Game> = OnceLock::new();
static ORIGINAL_FRAME_STAGE: AtomicUsize = AtomicUsize::new(0);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static PLAYING: AtomicBool = AtomicBool::new(false);
/// The demo's tick at the last rendered frame ([`NO_DEMO`]: none playing).
static DEMO_TICK: AtomicI64 = AtomicI64::new(NO_DEMO);
static STAGES: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());

/// The DLL's entry point. On load: refuses (returns 0, so Windows unloads it) in a CS2 without `-insecure`;
/// otherwise starts [`start`] on a thread of its own (nothing may wait on the loader lock there). A thread that cannot
/// be created leaves the DLL idle.
#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return 1;
    }
    log::open();
    log::line(&format!("ofast-capture {}", env!("CARGO_PKG_VERSION")));
    let line = command_line();
    if !gate::is_insecure(&line) {
        log::line(&format!(
            "REFUSED: CS2 was not started with -insecure, not loaded ({line})"
        ));
        return 0;
    }
    log::line("-insecure: yes");
    let _ = std::thread::Builder::new()
        .name("ofast-capture".into())
        .spawn(start);
    1
}

/// The process's own command line.
fn command_line() -> String {
    // SAFETY: GetCommandLineW returns the process's nul-terminated command line, valid for its life.
    unsafe {
        let line = GetCommandLineW();
        let mut length = 0;
        while *line.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(line, length))
    }
}

fn start() {
    match install() {
        Ok(()) => log::line("hooked: FrameStageNotify"),
        Err(e) => {
            log::line(&format!("NOT HOOKED: {e}"));
            return;
        }
    }
    match crate::capture::install() {
        Ok(()) => log::line("hooked: Present (frame capture)"),
        Err(e) => log::line(&format!("NO FRAME CAPTURE: {e}")),
    }
}

fn install() -> Result<(), String> {
    let started = Instant::now();
    let (client_module, engine_module) = loop {
        if let (Some(c), Some(e)) = (Module::find("client.dll"), Module::find("engine2.dll")) {
            break (c, e);
        }
        if started.elapsed() > WAIT_FOR_MODULES {
            return Err("client.dll or engine2.dll never loaded".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let client = client_module.interface(cs2::CLIENT_INTERFACE)?;
    let engine = engine_module.interface(cs2::ENGINE_INTERFACE)?;
    client.slot(cs2::CLIENT_FRAME_STAGE_NOTIFY, &client_module)?;
    engine.slot(cs2::ENGINE_IS_PLAYING_DEMO, &engine_module)?;
    engine.slot(cs2::ENGINE_GET_DEMO_FILE, &engine_module)?;
    let globals = cs2::globals_slot(&client, &client_module)?;
    let now = cs2::curtime(globals);
    if now.is_some_and(|t| !t.is_finite() || t < 0.0) {
        return Err(format!("the decoded clock reads {now:?}: not a clock"));
    }
    match crate::view::find_site(&client_module, engine.object as usize) {
        Ok(()) => log::line("view setup found: OFAST's cameras can play"),
        Err(e) => log::line(&format!("NO CAMERAS: {e}")),
    }
    log::line(&format!(
        "client.dll {:#x}, engine2.dll {:#x}, globals pointer kept at {:#x}, curtime now {now:?}",
        client_module.handle, engine_module.handle, globals as usize,
    ));
    let _ = GAME.set(Game {
        engine,
        engine_module,
        globals: globals as usize,
    });
    // SAFETY: slot 36 of the client's vtable is FrameStageNotify(this, stage) (checked to be client code), which
    // `frame_stage_notify` matches and calls on through ORIGINAL_FRAME_STAGE; the DLL is never unloaded.
    unsafe {
        hook::swap_slot(
            client.vtable,
            cs2::CLIENT_FRAME_STAGE_NOTIFY,
            frame_stage_notify as extern "system" fn(*mut c_void, i32) as usize,
            &ORIGINAL_FRAME_STAGE,
        )
    }
}

/// OFAST's `FrameStageNotify`: its work, then CS2's own.
extern "system" fn frame_stage_notify(this: *mut c_void, stage: i32) {
    // A panic must never cross into CS2.
    let _ = std::panic::catch_unwind(|| on_frame_stage(stage));
    let original = ORIGINAL_FRAME_STAGE.load(Ordering::Acquire);
    // SAFETY: the slot's own function, kept by `swap_slot` before the swap; same signature.
    let original: extern "system" fn(*mut c_void, i32) = unsafe { std::mem::transmute(original) };
    original(this, stage);
}

/// Whether a demo plays: the engine asked (for the view setup's patch, on the game's thread, which answers the
/// engine's question in its stead).
pub(crate) fn is_playing_demo() -> bool {
    GAME.get().is_some_and(|game| {
        game.engine
            .slot(cs2::ENGINE_IS_PLAYING_DEMO, &game.engine_module)
            .is_ok_and(|f| {
                // SAFETY: the engine's IsPlayingDemo(this) -> bool, a checked slot.
                let f: extern "system" fn(*mut c_void) -> u8 = unsafe { std::mem::transmute(f) };
                f(game.engine.object) != 0
            })
    })
}

/// The demo's tick at the last rendered frame, while a demo plays (safe from any thread: no engine call).
pub(crate) fn demo_tick() -> Option<i32> {
    let tick = DEMO_TICK.load(Ordering::Acquire);
    (tick != NO_DEMO).then_some(tick as i32)
}

/// The game's clock, seconds.
pub(crate) fn curtime() -> Option<f32> {
    GAME.get()
        .and_then(|game| cs2::curtime(game.globals as *const *const u8))
}

fn on_frame_stage(stage: i32) {
    crate::view::patch_if_pending();
    if STAGES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(stage)
    {
        log::line(&format!("frame stage {stage} seen"));
    }
    if stage != RENDER_STAGE {
        return;
    }
    let Some(game) = GAME.get() else { return };
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
    let tick = cs2::demo_tick(&game.engine, &game.engine_module);
    DEMO_TICK.store(tick.map_or(NO_DEMO, i64::from), Ordering::Release);
    let was_playing = PLAYING.swap(tick.is_some(), Ordering::Relaxed);
    if was_playing != tick.is_some() {
        log::line(if tick.is_some() {
            "demo playing"
        } else {
            "no demo playing"
        });
    }
    let (Some(tick), true) = (tick, frame.is_multiple_of(LOG_EVERY)) else {
        return;
    };
    match cs2::curtime(game.globals as *const *const u8) {
        Some(curtime) => {
            let game_tick = f64::from(curtime) * TICK_RATE;
            log::line(&format!(
                "frame {frame}: demo tick {tick}, curtime {curtime:.4} s = tick {game_tick:.2}, game tick - demo tick = {:.2}",
                game_tick - f64::from(tick)
            ));
        }
        None => log::line(&format!("frame {frame}: demo tick {tick}, no globals yet")),
    }
}
