//! Frames out of CS2 (M8, spike 2): the swap chain's `Present` hooked, each frame asked for copied from the back
//! buffer into a texture the CPU can read, and written as an image (TGA: 32-bit, top-down, no compression; an image
//! sequence any editor or FFmpeg reads).
//!
//! The hook: a throwaway Direct3D 11 device and swap chain on a hidden window give the address of `dxgi.dll`'s swap
//! chain vtable, which every swap chain of the process shares, CS2's too; its `Present` slot (8) is swapped for
//! OFAST's (checked to be `dxgi.dll` code first).
//!
//! A recording is asked for with a command file, `%LOCALAPPDATA%\OFAST\capture\command.txt`, holding
//! `record <frames> <folder>`: the next `<frames>` presented frames go to `<folder>\frame_00000.tga` …; the file is
//! read and deleted within half a second. The same file loads a camera to play (`camera <cameras file> <number>`,
//! `camera <cameras file> eyes <controller>`, `camera off`: [`crate::view`]), one command a line. For frames at an even pace of demo time, set CS2's `host_framerate` first
//! (`host_framerate 60`: each frame moves the demo 1/60 s however long it takes to draw and save).

use std::ffi::c_void;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDeviceAndSwapChain,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R10G10B10A2_UNORM,
    DXGI_MODE_DESC, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_SWAP_CHAIN_DESC, DXGI_SWAP_EFFECT_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGISwapChain,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPED,
};
use windows::core::{Interface, w};

use crate::cs2::Module;
use crate::{hook, log};

/// `IDXGISwapChain::Present`'s slot.
const PRESENT: usize = 8;

/// Every how many presents the command file is looked for (about twice a second).
const COMMAND_EVERY: u64 = 30;

static ORIGINAL_PRESENT: AtomicUsize = AtomicUsize::new(0);
static PRESENTS: AtomicU64 = AtomicU64::new(0);
static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

/// A recording under way.
struct Recording {
    folder: PathBuf,
    /// The demo tick to start at, if any (`record … from <tick>`).
    start_tick: Option<i32>,
    /// Frames still to write.
    left: u32,
    /// The next frame's number.
    frame: u32,
    /// The texture frames are copied into, and the device, size and format it was made for (another swap chain's
    /// device, or a new size, makes a new one).
    staging: Option<(ID3D11Texture2D, usize, u32, u32, DXGI_FORMAT)>,
}

/// The command file.
fn command_path() -> PathBuf {
    crate::log::path()
        .with_file_name("capture")
        .join("command.txt")
}

/// Hooks `Present` (module docs).
pub fn install() -> Result<(), String> {
    let dxgi = Module::find("dxgi.dll").ok_or("dxgi.dll is not loaded")?;
    // SAFETY: a hidden window of the built-in STATIC class, and a device and swap chain on it, all released before
    // returning; only the vtable's address is kept, which lives in dxgi.dll.
    unsafe {
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("ofast-capture"),
            WS_OVERLAPPED,
            0,
            0,
            64,
            64,
            None,
            None,
            None,
            None,
        )
        .map_err(|e| format!("no window for the throwaway swap chain: {e}"))?;
        let result = present_vtable(window);
        let _ = DestroyWindow(window);
        let vtable = result?;
        let present = *vtable.add(PRESENT);
        if !dxgi.in_code(present) {
            return Err(format!("Present {present:#x} is not dxgi.dll code"));
        }
        hook::swap_slot(
            vtable,
            PRESENT,
            present_hook as extern "system" fn(*mut c_void, u32, u32) -> i32 as usize,
            &ORIGINAL_PRESENT,
        )?;
    }
    let _ = std::fs::remove_file(command_path());
    Ok(())
}

/// The vtable of a throwaway swap chain on `window`.
unsafe fn present_vtable(window: HWND) -> Result<*mut usize, String> {
    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC {
            Width: 64,
            Height: 64,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ..Default::default()
        },
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 1,
        OutputWindow: window,
        Windowed: true.into(),
        SwapEffect: DXGI_SWAP_EFFECT_DISCARD,
        Flags: 0,
    };
    let mut swap_chain: Option<IDXGISwapChain> = None;
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: valid descriptions and out-parameters.
    unsafe {
        D3D11CreateDeviceAndSwapChain(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&desc),
            Some(&mut swap_chain),
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| format!("no throwaway swap chain: {e}"))?;
        let swap_chain = swap_chain.ok_or("no throwaway swap chain")?;
        Ok(*(swap_chain.as_raw() as *const *mut usize))
    }
}

/// OFAST's `Present`: its work, then the swap chain's own.
extern "system" fn present_hook(this: *mut c_void, sync_interval: u32, flags: u32) -> i32 {
    let _ = std::panic::catch_unwind(|| on_present(this));
    let original = ORIGINAL_PRESENT.load(Ordering::Acquire);
    // SAFETY: the slot's own function, kept by `install` before the swap; same signature.
    let original: extern "system" fn(*mut c_void, u32, u32) -> i32 =
        unsafe { std::mem::transmute(original) };
    original(this, sync_interval, flags)
}

fn on_present(this: *mut c_void) {
    let count = PRESENTS.fetch_add(1, Ordering::Relaxed);
    if count == 0 {
        log::line("Present seen");
    }
    let mut recording = RECORDING.lock().unwrap_or_else(PoisonError::into_inner);
    if recording.is_none() && count.is_multiple_of(COMMAND_EVERY) {
        *recording = read_command();
    }
    let Some(rec) = recording.as_mut() else {
        return;
    };
    // Not yet: the demo has not reached the first tick asked for.
    if let Some(start) = rec.start_tick
        && crate::dll::demo_tick().is_none_or(|tick| tick < start)
    {
        return;
    }
    // SAFETY: `this` is the swap chain Present was called on, alive for the call.
    let Some(swap_chain) = (unsafe { IDXGISwapChain::from_raw_borrowed(&this) }) else {
        return;
    };
    match capture_frame(swap_chain, rec) {
        Ok(()) => {
            rec.frame += 1;
            rec.left -= 1;
            if rec.left == 0 {
                log::line(&format!(
                    "recorded {} frames into {}",
                    rec.frame,
                    rec.folder.display()
                ));
                *recording = None;
            }
        }
        Err(e) => {
            log::line(&format!("recording stopped: {e}"));
            *recording = None;
        }
    }
}

/// Carries out the command file's lines, if there is one (the file deleted): `camera …` lines at once, and the
/// recording a `record` line asks for is returned.
fn read_command() -> Option<Recording> {
    let path = command_path();
    let bytes = std::fs::read(&path).ok()?;
    // Deleted whatever it holds: a file that does not read would otherwise be read again and again.
    let _ = std::fs::remove_file(&path);
    let Some(text) = command_text(&bytes) else {
        log::line("command file not read: UTF-8 or UTF-16 text expected");
        return None;
    };
    let mut recording = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("camera ") {
            camera_command(rest.trim());
        } else {
            recording = record_command(line).or(recording);
        }
    }
    recording
}

/// A command file's text: UTF-8 (a byte order mark dropped) or UTF-16 with its byte order mark (what Windows
/// PowerShell 5's `>` writes).
fn command_text(bytes: &[u8]) -> Option<String> {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16(&units).ok()
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    }
}

/// A path as written, double quotes around it dropped.
fn unquoted(path: &str) -> &str {
    path.trim()
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path.trim())
}

/// `camera off`, `camera <cameras file> <number>`, `camera <cameras file> eyes <controller>` (the file's path may
/// hold spaces: the last one or two words are the camera).
fn camera_command(rest: &str) {
    if rest == "off" {
        crate::view::unload();
        return;
    }
    let words: Vec<&str> = rest.split(' ').collect();
    let (file, which) = match words[..] {
        [.., "eyes", controller] if words.len() > 2 => (
            words[..words.len() - 2].join(" "),
            format!("eyes {controller}"),
        ),
        [.., number] if words.len() > 1 => (words[..words.len() - 1].join(" "), number.to_owned()),
        _ => {
            log::line(&format!(
                "camera command not understood: {rest:?} (camera <file> <number> | camera <file> eyes <controller> | camera off)"
            ));
            return;
        }
    };
    if let Err(e) = crate::view::load(std::path::Path::new(unquoted(&file)), &which) {
        log::line(&format!("camera not loaded: {e}"));
    }
}

/// `record <frames> <folder>`, or `record <frames> from <tick> <folder>`: from the first frame at or past that demo
/// tick (CS2 under `host_framerate` runs a demo faster than real time: the tick, not a delay, says when).
fn record_command(line: &str) -> Option<Recording> {
    let mut words = line.splitn(3, ' ');
    let parsed = match (
        words.next(),
        words.next().map(str::parse::<u32>),
        words.next(),
    ) {
        (Some("record"), Some(Ok(frames)), Some(rest)) if frames > 0 => {
            match rest.split_once(' ') {
                Some(("from", rest)) => rest.split_once(' ').and_then(|(tick, folder)| {
                    Some((
                        frames,
                        Some(tick.parse::<i32>().ok()?),
                        PathBuf::from(unquoted(folder)),
                    ))
                }),
                _ => Some((frames, None, PathBuf::from(unquoted(rest)))),
            }
        }
        _ => None,
    };
    let Some((frames, start_tick, folder)) = parsed else {
        log::line(&format!(
            "command not understood: {line:?} (record <frames> [from <tick>] <folder>)"
        ));
        return None;
    };
    // OFAST films demos: a recording waits for a demo's tick, or starts only while one plays.
    if start_tick.is_none() && crate::dll::demo_tick().is_none() {
        log::line("not recording: no demo is playing");
        return None;
    }
    if let Err(e) = std::fs::create_dir_all(&folder) {
        log::line(&format!("{}: {e}", folder.display()));
        return None;
    }
    match start_tick {
        Some(tick) => log::line(&format!(
            "recording {frames} frames into {} from demo tick {tick}",
            folder.display()
        )),
        None => log::line(&format!(
            "recording {frames} frames into {}",
            folder.display()
        )),
    }
    Some(Recording {
        folder,
        left: frames,
        frame: 0,
        start_tick,
        staging: None,
    })
}

/// Copies the back buffer through the staging texture and writes it as the recording's next image.
fn capture_frame(swap_chain: &IDXGISwapChain, rec: &mut Recording) -> Result<(), String> {
    // SAFETY: Direct3D 11 calls on live objects, from the thread presenting (the immediate context's).
    unsafe {
        let back: ID3D11Texture2D = swap_chain
            .GetBuffer(0)
            .map_err(|e| format!("GetBuffer: {e}"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        back.GetDesc(&mut desc);
        let device = back.GetDevice().map_err(|e| format!("GetDevice: {e}"))?;
        let context = device
            .GetImmediateContext()
            .map_err(|e| format!("GetImmediateContext: {e}"))?;
        let fresh = rec.staging.as_ref().is_none_or(|(_, d, w, h, f)| {
            (*d, *w, *h, *f)
                != (
                    device.as_raw() as usize,
                    desc.Width,
                    desc.Height,
                    desc.Format,
                )
        });
        if fresh {
            if rec.frame == 0 {
                log::line(&format!(
                    "back buffer {} x {}, format {}, {} samples",
                    desc.Width, desc.Height, desc.Format.0, desc.SampleDesc.Count
                ));
            }
            let staging_desc = D3D11_TEXTURE2D_DESC {
                MipLevels: 1,
                ArraySize: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            let mut staging = None;
            device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| format!("CreateTexture2D: {e}"))?;
            let staging = staging.ok_or("no staging texture")?;
            rec.staging = Some((
                staging,
                device.as_raw() as usize,
                desc.Width,
                desc.Height,
                desc.Format,
            ));
        }
        if desc.SampleDesc.Count != 1 {
            return Err("the back buffer is multisampled".into());
        }
        let (staging, _, width, height, format) = rec.staging.as_ref().expect("made above");
        context.CopyResource(staging, &back);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("Map: {e}"))?;
        let rows = std::slice::from_raw_parts(
            mapped.pData as *const u8,
            mapped.RowPitch as usize * *height as usize,
        );
        let pixels = to_bgra(
            rows,
            mapped.RowPitch as usize,
            *width as usize,
            *height as usize,
            *format,
        );
        context.Unmap(staging, 0);
        let pixels = pixels?;
        let path = rec.folder.join(format!("frame_{:05}.tga", rec.frame));
        write_tga(&path, *width as u16, *height as u16, &pixels)
            .map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Mapped rows of a back buffer as tightly packed BGRA, alpha opaque.
fn to_bgra(
    rows: &[u8],
    pitch: usize,
    width: usize,
    height: usize,
    format: DXGI_FORMAT,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = &rows[y * pitch..y * pitch + width * 4];
        for px in row.as_chunks::<4>().0 {
            let bgra = match format {
                DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => {
                    [px[0], px[1], px[2], 255]
                }
                DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
                    [px[2], px[1], px[0], 255]
                }
                DXGI_FORMAT_R10G10B10A2_UNORM => {
                    let v = u32::from_le_bytes(*px);
                    let c = |shift: u32| ((v >> shift) & 0x3FF) as f32 / 1023.0 * 255.0 + 0.5;
                    [c(20) as u8, c(10) as u8, c(0) as u8, 255]
                }
                other => return Err(format!("back buffer format {} is not handled", other.0)),
            };
            out.extend_from_slice(&bgra);
        }
    }
    Ok(out)
}

/// A 32-bit uncompressed TGA, top row first.
fn write_tga(path: &std::path::Path, width: u16, height: u16, bgra: &[u8]) -> std::io::Result<()> {
    let mut header = [0u8; 18];
    header[2] = 2; // uncompressed true colour
    header[12..14].copy_from_slice(&width.to_le_bytes());
    header[14..16].copy_from_slice(&height.to_le_bytes());
    header[16] = 32;
    header[17] = 0x28; // 8 alpha bits, origin top left
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(&header)?;
    file.write_all(bgra)?;
    file.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_files_read_in_utf8_and_utf16() {
        assert_eq!(command_text(b"camera off").as_deref(), Some("camera off"));
        assert_eq!(
            command_text(b"\xEF\xBB\xBFcamera off").as_deref(),
            Some("camera off")
        );
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("camera off".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        assert_eq!(command_text(&utf16).as_deref(), Some("camera off"));
        assert!(command_text(&[0xC3, 0x28]).is_none());
    }

    #[test]
    fn quotes_around_paths_go() {
        assert_eq!(unquoted(r#" "C:\a b\c.json" "#), r"C:\a b\c.json");
        assert_eq!(unquoted(r"C:\a\c.json"), r"C:\a\c.json");
    }

    #[test]
    fn ten_bit_pixels_become_eight() {
        // Red 1023 (bits 0-9), green 0, blue 512 (bits 20-29): BGRA 128, 0, 255, 255.
        let v: u32 = 1023 | (512 << 20);
        let bgra = to_bgra(&v.to_le_bytes(), 4, 1, 1, DXGI_FORMAT_R10G10B10A2_UNORM).unwrap();
        assert_eq!(bgra, vec![128, 0, 255, 255]);
    }
}
