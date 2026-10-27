//! What ofast-capture knows of CS2's insides, all in one place and each checked before use (decision 022).
//!
//! | What | Where | How it is checked |
//! |---|---|---|
//! | The client's interface | `client.dll`'s `CreateInterface("Source2Client002")` | non-null, its vtable in `client.dll` |
//! | The engine's interface | `engine2.dll`'s `CreateInterface("Source2EngineToClient001")` | non-null, its vtable in `engine2.dll` |
//! | `SetGlobals` | client slot 11 | its code in `client.dll`'s code; stores `rdx` to a global ([`super::x64::rdx_store`]) in `client.dll`'s image |
//! | `FrameStageNotify` | client slot 36 | its code in `client.dll`'s code |
//! | `IsPlayingDemo` | engine slot 42 | its code in `engine2.dll`'s code |
//! | `GetDemoFile` | engine slot 69, then the demo file's slot 3 `GetDemoTick` | non-null while a demo plays |
//! | `curtime` | the globals' 13th 4-byte field (`+0x30`) | moves while a demo plays |
//!
//! The slot numbers and the clock's place are HLAE's (`AfxHookSource2`, its headers of CS2's interfaces, CS2's
//! 2026-09-23 build; docs/research/hlae-cs2-capture.md). A check that fails stops the DLL before it changes anything.

use std::ffi::c_void;
use std::ops::Range;

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE,
    PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY,
    VirtualQuery,
};

use crate::x64;

/// The client interface's version.
pub const CLIENT_INTERFACE: &str = "Source2Client002";
/// The engine interface's version.
pub const ENGINE_INTERFACE: &str = "Source2EngineToClient001";
/// Client slot of `SetGlobals(CGlobalVars*)`.
pub const CLIENT_SET_GLOBALS: usize = 11;
/// Client slot of `FrameStageNotify(ClientFrameStage_t)`.
pub const CLIENT_FRAME_STAGE_NOTIFY: usize = 36;
/// Engine slot of `IsPlayingDemo()`.
pub const ENGINE_IS_PLAYING_DEMO: usize = 42;
/// Engine slot of `GetDemoFile()`.
pub const ENGINE_GET_DEMO_FILE: usize = 69;
/// Demo file slot of `GetDemoTick()`.
pub const DEMO_FILE_GET_DEMO_TICK: usize = 3;
/// Byte offset of `curtime` (f32) in the globals.
pub const GLOBALS_CURTIME: usize = 0x30;

/// A module of CS2 in memory: where its image lies, and its code.
#[derive(Debug, Clone)]
pub struct Module {
    /// Its name.
    pub name: &'static str,
    /// Its handle (base address).
    pub handle: usize,
    /// The whole image.
    pub image: Range<usize>,
    /// Its executable sections.
    pub code: Vec<Range<usize>>,
    /// Its writable sections (its global variables).
    pub data: Vec<Range<usize>>,
}

impl Module {
    /// A loaded module by name (`client.dll`), with its sections read from its PE headers; `None` before it is
    /// loaded.
    pub fn find(name: &'static str) -> Option<Module> {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: a valid, nul-terminated wide string.
        let handle = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
        if handle == 0 {
            return None;
        }
        // SAFETY: a loaded module's headers are mapped and readable at its base; the offsets are the PE format's.
        unsafe {
            let base = handle as *const u8;
            let nt = base.add(read_u32(base, 0x3C) as usize);
            if read_u32(nt, 0) != 0x0000_4550 {
                return None;
            }
            let sections = usize::from(read_u16(nt, 6));
            let optional_size = usize::from(read_u16(nt, 20));
            let image_size = read_u32(nt, 24 + 56) as usize;
            let table = nt.add(24 + optional_size);
            // Sections with any of `flags` in their characteristics, as address ranges.
            let sections_with = |flags: u32| -> Vec<Range<usize>> {
                (0..sections)
                    .map(|i| table.add(i * 40))
                    .filter(|s| read_u32(*s, 36) & flags != 0)
                    .map(|s| {
                        let start = handle + read_u32(s, 12) as usize;
                        start..start + read_u32(s, 8) as usize
                    })
                    .collect()
            };
            Some(Module {
                name,
                handle,
                image: handle..handle + image_size,
                code: sections_with(0x2000_0000), // IMAGE_SCN_MEM_EXECUTE
                data: sections_with(0x8000_0000), // IMAGE_SCN_MEM_WRITE
            })
        }
    }

    /// Whether an address is in its code.
    pub fn in_code(&self, address: usize) -> bool {
        self.code.iter().any(|c| c.contains(&address))
    }

    /// An interface it exports through `CreateInterface`, checked: non-null, its vtable inside this module.
    pub fn interface(&self, version: &str) -> Result<Interface, String> {
        // SAFETY: a loaded module's handle and a nul-terminated name.
        let export =
            unsafe { GetProcAddress(self.handle as _, c"CreateInterface".as_ptr().cast()) }
                .ok_or_else(|| format!("{} has no CreateInterface", self.name))?;
        // SAFETY: Source 2's CreateInterface(const char *name, int *returnCode) -> void *.
        let create: extern "C" fn(*const u8, *mut i32) -> *mut c_void =
            unsafe { std::mem::transmute(export) };
        let name: Vec<u8> = version.bytes().chain([0]).collect();
        let object = create(name.as_ptr(), std::ptr::null_mut());
        if object.is_null() {
            return Err(format!("{} gave no {version}", self.name));
        }
        // SAFETY: an interface object starts with its vtable pointer.
        let vtable = unsafe { *(object as *const usize) };
        if !self.image.contains(&vtable) {
            return Err(format!(
                "{version}'s vtable {vtable:#x} is not in {}",
                self.name
            ));
        }
        Ok(Interface {
            object,
            vtable: vtable as *mut usize,
        })
    }
}

/// An interface object of CS2's and its vtable.
#[derive(Debug, Clone, Copy)]
pub struct Interface {
    /// The object.
    pub object: *mut c_void,
    /// Its vtable.
    pub vtable: *mut usize,
}

// SAFETY: interfaces are process-wide singletons; OFAST calls them from the game's own thread (the frame hook).
unsafe impl Send for Interface {}
// SAFETY: as above.
unsafe impl Sync for Interface {}

impl Interface {
    /// The function in a slot, checked to lie in `module`'s code.
    pub fn slot(&self, index: usize, module: &Module) -> Result<usize, String> {
        // SAFETY: the vtable is in the module's image (Module::interface), and CS2's vtables are longer than the
        // slots OFAST reads.
        let function = unsafe { *self.vtable.add(index) };
        if module.in_code(function) {
            Ok(function)
        } else {
            Err(format!(
                "slot {index}: {function:#x} is not in {}'s code",
                module.name
            ))
        }
    }
}

unsafe fn read_u32(base: *const u8, offset: usize) -> u32 {
    // SAFETY: the caller's (readable memory at base + offset).
    unsafe { (base.add(offset) as *const u32).read_unaligned() }
}

unsafe fn read_u16(base: *const u8, offset: usize) -> u16 {
    // SAFETY: the caller's.
    unsafe { (base.add(offset) as *const u16).read_unaligned() }
}

/// Whether `len` bytes from `address` are committed memory that can be read (not a guard page): checked before every
/// read through a pointer CS2 hands over, so a wrong pointer is refused rather than faulting the game.
pub fn readable(address: usize, len: usize) -> bool {
    if address == 0 {
        return false;
    }
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: VirtualQuery only describes the page; it reads nothing at the address.
    let size = unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    let readable_protection = PAGE_READONLY
        | PAGE_READWRITE
        | PAGE_WRITECOPY
        | PAGE_EXECUTE_READ
        | PAGE_EXECUTE_READWRITE
        | PAGE_EXECUTE_WRITECOPY;
    size != 0
        && info.State == MEM_COMMIT
        && info.Protect & readable_protection != 0
        && info.Protect & PAGE_GUARD == 0
        && address + len <= info.BaseAddress as usize + info.RegionSize
}

/// Where the client keeps the engine's globals pointer: the global `SetGlobals` stores its argument to (through one
/// jump thunk at most) before its first `ret`, checked to lie in one of the client's writable sections.
pub fn globals_slot(client: &Interface, module: &Module) -> Result<*const *const u8, String> {
    let mut function = client.slot(CLIENT_SET_GLOBALS, module)?;
    // SAFETY: a function in the module's code: its first bytes are readable.
    let mut code = unsafe { std::slice::from_raw_parts(function as *const u8, 64) };
    if let Some(target) = x64::jump_target(code, function).filter(|t| module.in_code(*t)) {
        function = target;
        // SAFETY: as above.
        code = unsafe { std::slice::from_raw_parts(function as *const u8, 64) };
    }
    let slot = x64::rdx_store(code, function).ok_or_else(|| {
        format!(
            "SetGlobals stores no rdx before its end: {}",
            hex(&code[..32])
        )
    })?;
    if !module.data.iter().any(|d| d.contains(&slot)) {
        return Err(format!(
            "SetGlobals stores to {slot:#x}, not a global variable of client.dll"
        ));
    }
    Ok(slot as *const *const u8)
}

/// The game's clock, seconds (`curtime`), from the globals pointer kept at `slot`; `None` before the engine set it,
/// or when the pointer does not lead to readable memory.
pub fn curtime(slot: *const *const u8) -> Option<f32> {
    // SAFETY: the slot is a global variable of client.dll (globals_slot); the block it points to is checked readable
    // before the read.
    unsafe {
        let globals = slot.read_volatile();
        let at = (globals as usize).checked_add(GLOBALS_CURTIME)?;
        readable(at, 4).then(|| (at as *const f32).read_volatile())
    }
}

/// The demo's tick while one plays, else `None`. Call it on the game's own thread only (the engine's demo file can
/// go away between two calls from elsewhere).
pub fn demo_tick(engine: &Interface, module: &Module) -> Option<i32> {
    let playing = engine.slot(ENGINE_IS_PLAYING_DEMO, module).ok()?;
    let get_file = engine.slot(ENGINE_GET_DEMO_FILE, module).ok()?;
    // SAFETY: engine methods taking only `this` (x64: rcx), returning a bool / a pointer (checked slots); the demo
    // file's vtable checked to be in the engine before its slot is read, and the slot to be engine code.
    unsafe {
        let playing: extern "system" fn(*mut c_void) -> u8 = std::mem::transmute(playing);
        if playing(engine.object) == 0 {
            return None;
        }
        let get_file: extern "system" fn(*mut c_void) -> *mut c_void =
            std::mem::transmute(get_file);
        let file = get_file(engine.object);
        if !readable(file as usize, 8) {
            return None;
        }
        let vtable = *(file as *const usize);
        let slot = vtable.checked_add(DEMO_FILE_GET_DEMO_TICK * 8)?;
        if !module.image.contains(&vtable) || !module.image.contains(&slot) {
            return None;
        }
        let function = *(slot as *const usize);
        if !module.in_code(function) {
            return None;
        }
        let tick: extern "system" fn(*mut c_void) -> i32 = std::mem::transmute(function);
        Some(tick(file))
    }
}

/// Bytes as hex, for the log.
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
