//! OFAST's cameras inside CS2 (M8, spike 3): the view CS2 draws a frame with, replaced by a camera of a cameras file
//! at the game's clock (decision 022).
//!
//! Where: `CViewRender::SetUpView` in `client.dll` asks the engine whether a demo plays (`IsPlayingDemo`, engine
//! slot 42) with the view setup it is filling in register `rbx`: `mov rcx, [engine]; mov rax, [rcx];
//! call [rax + 42 × 8]; xorps xmm7, xmm7; test al, al; je …; lea rcx, …` (HLAE's finding, CS2's 2026-09-23 build:
//! docs/research/hlae-cs2-capture.md). Those 16 bytes, found by pattern exactly once and checked byte for byte, become
//! `mov rcx, rbx; mov rax, view_setup; call rax; nop` ([`x64::call_with_rbx`]): OFAST's [`view_setup`] gets the view
//! setup, may change it, and answers the engine's question itself, so CS2 goes on as before. The patch goes in from
//! the game's own thread, between frames (`patch_if_pending`, called from the frame hook).
//!
//! Before the patch, the call's `mov rcx, [global]` must load the engine interface OFAST found itself (the slot-42
//! call is then the engine's `IsPlayingDemo`, which OFAST answers by asking the same engine).
//!
//! The view setup's fields (HLAE's offsets): width and height (`i32`, `0x434`, `0x43C`), field of view (`f32`, CS2's:
//! horizontal at 4:3, `0x498`), origin (3 × `f32`, `0x4A0`), angles (pitch, yaw, roll, 3 × `f32`, `0x4B8`). Every
//! call checks the memory is readable before reading it (a register that no longer holds a view setup would point
//! anywhere); on its first call the fields must look like a view — a picture size, a field of view between 1 and
//! 179°, finite angles with the pitch within ±90° — or OFAST never changes them, and says so in its log. Views change
//! only while a demo plays, the pitch kept within ±89° and the field of view within 1 – 170°.
//!
//! A camera is loaded with the command file (`camera <cameras file> <number>`, `camera <cameras file> eyes
//! <controller>`, `camera off`): each frame, the game's clock gives the demo tick (`curtime × 64 − server tick
//! offset`, the file's), and the camera's keys are interpolated there (position and field of view linearly,
//! rotation by spherical interpolation). Outside the camera's keys CS2's own view stays.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use ofast_core::camera::{CameraKey, Vec3, angles_to_quat, quat_to_angles, slerp};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::cs2::{self, Module};
use crate::{log, x64};

/// The view setup's fields (module docs).
const WIDTH: usize = 0x434;
const HEIGHT: usize = 0x43C;
const FOV: usize = 0x498;
const ORIGIN: usize = 0x4A0;
const ANGLES: usize = 0x4B8;

/// The patch site: where the view setup's 16 bytes were found (0: none).
static SITE: AtomicUsize = AtomicUsize::new(0);
/// The patch is in.
static PATCHED: AtomicBool = AtomicBool::new(false);
/// The view setup's first look: 0 not yet, 1 as expected, 2 not (never changed then).
static CHECKED: AtomicUsize = AtomicUsize::new(0);
static OVERRIDDEN: AtomicU64 = AtomicU64::new(0);
static CAMERA: Mutex<Option<Camera>> = Mutex::new(None);

/// A camera to play: its keys by tick and the demo's clock.
struct Camera {
    name: String,
    keys: Vec<CameraKey>,
    server_tick_offset: f64,
    /// Inside its keys on the last frame.
    active: bool,
}

/// The bytes the patch replaces and what follows them, `call [rax + slot × 8]` built from OFAST's own slot number.
fn site_pattern() -> x64::Pattern {
    let slot = (cs2::ENGINE_IS_PLAYING_DEMO * 8) as u32;
    let [a, b, c, d] = slot.to_le_bytes();
    let text = format!(
        "48 8b 0d ?? ?? ?? ?? 48 8b 01 ff 90 {a:02x} {b:02x} {c:02x} {d:02x} 0f 57 ff 84 c0 74 ?? 48 8d 0d"
    );
    x64::pattern(&text).expect("a well-formed pattern")
}

/// Finds the patch site in `client.dll`'s code: exactly one match, whose `mov rcx, [rip + disp32]` loads the engine
/// interface `engine` (so the slot-42 call it makes is the engine's IsPlayingDemo, whose answer OFAST gives in its
/// stead); else an error saying what was found.
pub fn find_site(client: &Module, engine: usize) -> Result<(), String> {
    let pattern = site_pattern();
    let mut found = Vec::new();
    for code in &client.code {
        // SAFETY: an executable section of a loaded module: mapped and readable.
        let bytes = unsafe { std::slice::from_raw_parts(code.start as *const u8, code.len()) };
        found.extend(
            x64::find(bytes, &pattern)
                .into_iter()
                .map(|at| code.start + at),
        );
    }
    let [site] = found[..] else {
        return Err(format!(
            "the view setup's call to IsPlayingDemo was found {} times (once expected): CS2 changed",
            found.len()
        ));
    };
    // SAFETY: the 7-byte `mov rcx, [rip + disp32]` at the site, in client.dll's code.
    let disp = unsafe { (site as *const u8).add(3).cast::<i32>().read_unaligned() };
    let global = (site as i64 + 7 + i64::from(disp)) as usize;
    if !client.data.iter().any(|d| d.contains(&global)) || !cs2::readable(global, 8) {
        return Err(format!(
            "the view setup's call reads {global:#x}, not a global of client.dll"
        ));
    }
    // SAFETY: a readable global variable of client.dll (checked just above).
    let loaded = unsafe { (global as *const usize).read_volatile() };
    if loaded != engine {
        return Err(format!(
            "the view setup asks {loaded:#x}, not the engine interface {engine:#x}: CS2 changed"
        ));
    }
    SITE.store(site, Ordering::Release);
    Ok(())
}

/// Puts the patch in, once, if a site was found: from the game's thread, between frames.
pub fn patch_if_pending() {
    let site = SITE.load(Ordering::Acquire);
    if site == 0 || PATCHED.swap(true, Ordering::AcqRel) {
        return;
    }
    let pattern = site_pattern();
    // SAFETY: 16 bytes at the site, found in client.dll's code; checked against the pattern again right before they
    // are written, under a temporary write permission; the instruction cache flushed after.
    unsafe {
        let bytes = std::slice::from_raw_parts(site as *const u8, 16);
        if x64::find(bytes, &pattern[..16]) != [0] {
            log::line("view setup: the bytes changed before the patch, not patched");
            return;
        }
        let code =
            x64::call_with_rbx(view_setup as extern "system" fn(*mut u8) -> u8 as usize as u64);
        let mut old: PAGE_PROTECTION_FLAGS = 0;
        if VirtualProtect(site as *const _, 16, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            log::line("view setup: VirtualProtect failed, not patched");
            return;
        }
        std::ptr::copy_nonoverlapping(code.as_ptr(), site as *mut u8, 16);
        let mut ignored: PAGE_PROTECTION_FLAGS = 0;
        VirtualProtect(site as *const _, 16, old, &mut ignored);
        FlushInstructionCache(GetCurrentProcess(), site as *const _, 16);
    }
    log::line(&format!("view setup patched at {site:#x}"));
}

/// OFAST's stand-in for the view setup's `IsPlayingDemo` call: the engine asked, and while a demo plays the view
/// possibly replaced; then the engine's answer.
extern "system" fn view_setup(view: *mut u8) -> u8 {
    let playing = crate::dll::is_playing_demo();
    if playing {
        let _ = std::panic::catch_unwind(|| override_view(view));
    }
    u8::from(playing)
}

/// The bytes of the view setup OFAST reads and writes: from its start to the end of its angles.
const VIEW_BYTES: usize = ANGLES + 12;

fn read<T: Copy>(view: *mut u8, offset: usize) -> T {
    // SAFETY: the view setup's fields at HLAE's offsets, checked readable (override_view) and to look like a view on
    // the first call.
    unsafe { (view.add(offset) as *const T).read_unaligned() }
}

fn write<T: Copy>(view: *mut u8, offset: usize, value: T) {
    // SAFETY: as `read`.
    unsafe { (view.add(offset) as *mut T).write_unaligned(value) }
}

/// Whether the fields look like a view (module docs).
fn plausible(view: *mut u8) -> bool {
    let (width, height): (i32, i32) = (read(view, WIDTH), read(view, HEIGHT));
    let fov: f32 = read(view, FOV);
    let angles: [f32; 3] = read(view, ANGLES);
    let origin: [f32; 3] = read(view, ORIGIN);
    (64..=16384).contains(&width)
        && (64..=16384).contains(&height)
        && fov > 1.0
        && fov < 179.0
        && angles.iter().chain(&origin).all(|v| v.is_finite())
        && angles[0].abs() <= 90.01
}

fn override_view(view: *mut u8) {
    // A register that no longer holds a view setup (CS2 changed) would point anywhere: never read it unchecked.
    if !cs2::readable(view as usize, VIEW_BYTES) {
        if CHECKED.swap(2, Ordering::AcqRel) != 2 {
            log::line(&format!(
                "view setup: {view:p} is not readable memory: OFAST will not change views"
            ));
        }
        return;
    }
    match CHECKED.load(Ordering::Acquire) {
        0 => {
            let ok = plausible(view);
            let (width, height): (i32, i32) = (read(view, WIDTH), read(view, HEIGHT));
            log::line(&format!(
                "view setup: {width} x {height}, fov {:.2}, origin {:?}, angles {:?}: {}",
                read::<f32>(view, FOV),
                read::<[f32; 3]>(view, ORIGIN),
                read::<[f32; 3]>(view, ANGLES),
                if ok {
                    "as expected"
                } else {
                    "NOT as expected: OFAST will not change views"
                }
            ));
            CHECKED.store(if ok { 1 } else { 2 }, Ordering::Release);
            if !ok {
                return;
            }
        }
        1 => {}
        _ => return,
    }
    let mut camera = CAMERA.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(camera) = camera.as_mut() else {
        return;
    };
    let Some(curtime) = crate::dll::curtime() else {
        return;
    };
    let tick = f64::from(curtime) * 64.0 - camera.server_tick_offset;
    let Some((origin, angles, fov)) = sample(&camera.keys, tick) else {
        if camera.active {
            camera.active = false;
            log::line(&format!(
                "camera {}: out of its keys at tick {tick:.2}",
                camera.name
            ));
        }
        return;
    };
    if !camera.active {
        camera.active = true;
        log::line(&format!(
            "camera {}: playing from tick {tick:.2}",
            camera.name
        ));
    }
    // Kept within what CS2 draws: a pitch past straight up or down, or a field of view of 0 or 180°, is no view.
    let angles = [angles[0].clamp(-89.0, 89.0), angles[1], angles[2]];
    write(view, ORIGIN, origin.map(|v| v as f32));
    write(view, ANGLES, angles.map(|v| v as f32));
    write(view, FOV, fov.clamp(1.0, 170.0) as f32);
    OVERRIDDEN.fetch_add(1, Ordering::Relaxed);
}

/// The camera at `tick` between its keys (none outside them): position and field of view linear, rotation spherical.
fn sample(keys: &[CameraKey], tick: f64) -> Option<(Vec3, Vec3, f64)> {
    let first = keys.first()?;
    let last = keys.last()?;
    if tick < first.tick || tick > last.tick {
        return None;
    }
    let i = keys
        .partition_point(|k| k.tick <= tick)
        .clamp(1, keys.len())
        - 1;
    let a = &keys[i];
    let Some(b) = keys.get(i + 1) else {
        return Some((a.position, a.angles, a.fov));
    };
    let t = if b.tick > a.tick {
        (tick - a.tick) / (b.tick - a.tick)
    } else {
        0.0
    };
    let lerp = |x: f64, y: f64| x + (y - x) * t;
    let position = std::array::from_fn(|k| lerp(a.position[k], b.position[k]));
    let rotation = slerp(angles_to_quat(a.angles), angles_to_quat(b.angles), t);
    Some((position, quat_to_angles(rotation), lerp(a.fov, b.fov)))
}

/// Loads a camera from a cameras file (`docs/formats/cameras.md`): `which` is a camera's number, or `eyes <controller>`
/// for a player's first-person camera.
pub fn load(file: &Path, which: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    let offset = json["server_tick_offset"]
        .as_f64()
        .ok_or("the cameras file has no server_tick_offset")?;
    let (name, keys) = if let Some(controller) = which.strip_prefix("eyes ") {
        let rows = json["eyes"][controller.trim()]
            .as_array()
            .ok_or_else(|| format!("no eyes for controller {controller}"))?;
        let keys = rows
            .iter()
            .filter_map(|r| {
                let v: Vec<f64> = r
                    .as_array()?
                    .iter()
                    .filter_map(serde_json::Value::as_f64)
                    .collect();
                (v.len() == 6).then(|| CameraKey {
                    tick: v[0],
                    position: [v[1], v[2], v[3]],
                    angles: [v[4], v[5], 0.0],
                    fov: ofast_core::camera::DEFAULT_FOV,
                })
            })
            .collect();
        (format!("eyes of {}", controller.trim()), keys)
    } else {
        let number: usize = which
            .trim()
            .parse()
            .map_err(|_| format!("{which:?}: a camera number or eyes <controller>"))?;
        let (_, cameras) = ofast_core::export::cameras::read_cameras(&text)?;
        let camera = cameras
            .into_iter()
            .nth(number)
            .ok_or_else(|| format!("no camera {number}"))?;
        (camera.name, camera.keys)
    };
    if keys.is_empty() {
        return Err(format!("{name}: no keys"));
    }
    log::line(&format!(
        "camera {name}: {} keys, ticks {:.0} - {:.0}",
        keys.len(),
        keys[0].tick,
        keys[keys.len() - 1].tick
    ));
    {
        let mut camera = CAMERA.lock().unwrap_or_else(PoisonError::into_inner);
        *camera = Some(Camera {
            name,
            keys,
            server_tick_offset: offset,
            active: false,
        });
    }
    Ok(())
}

/// Stops playing a camera: CS2's own view again.
pub fn unload() {
    if CAMERA
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .is_some()
    {
        log::line(&format!(
            "camera off ({} views changed)",
            OVERRIDDEN.load(Ordering::Relaxed)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(tick: f64, x: f64, yaw: f64) -> CameraKey {
        CameraKey {
            tick,
            position: [x, 0.0, 0.0],
            angles: [0.0, yaw, 0.0],
            fov: 90.0,
        }
    }

    #[test]
    fn cameras_are_sampled_between_their_keys_only() {
        let keys = [key(10.0, 0.0, 170.0), key(12.0, 20.0, -170.0)];
        assert!(sample(&keys, 9.9).is_none());
        assert!(sample(&keys, 12.1).is_none());
        let (p, a, _) = sample(&keys, 11.0).unwrap();
        assert!((p[0] - 10.0).abs() < 1e-9);
        // Halfway between 170° and −170° the short way: 180°.
        assert!((ofast_core::camera::wrap_degrees(a[1] - 180.0)).abs() < 1e-9);
        assert_eq!(sample(&keys, 12.0).unwrap().0[0], 20.0);
    }

    #[test]
    fn the_site_pattern_calls_slot_42() {
        let p = site_pattern();
        assert_eq!(p.len(), 26);
        assert_eq!(
            &p[10..16],
            &[
                Some(0xff),
                Some(0x90),
                Some(0x50),
                Some(0x01),
                Some(0),
                Some(0)
            ]
        );
    }
}
