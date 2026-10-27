//! Attaching the DLL to a running CS2 (decision 022): find `cs2.exe`, read its command line and refuse without
//! `-insecure` ([`crate::gate`]), refuse when OFAST is in it already, then have CS2 load a copy of the DLL
//! (`CreateRemoteThread` on `LoadLibraryW`). The copy (`%LOCALAPPDATA%\OFAST\capture\`) leaves the built DLL free to
//! be built again while CS2 holds the copy.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use windows_sys::Wdk::System::Threading::NtQueryInformationProcess;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W,
    Process32FirstW, Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_CREATE_THREAD,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_OPERATION,
    PROCESS_VM_READ, PROCESS_VM_WRITE, WaitForSingleObject,
};

use crate::gate;

/// `ProcessCommandLineInformation` (Windows 8.1 and later).
const PROCESS_COMMAND_LINE_INFORMATION: i32 = 60;

/// The DLL's file name, as cargo builds it.
pub const DLL_NAME: &str = "ofast_capture.dll";

/// What an attach found and did.
#[derive(Debug, Clone)]
pub struct Attached {
    /// The CS2 process.
    pub pid: u32,
    /// Its command line (it has `-insecure`).
    pub command_line: String,
    /// The copy of the DLL CS2 loaded.
    pub dll: PathBuf,
}

/// A process handle closed when dropped.
struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: a handle OpenProcess gave and nobody else closes.
        unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32, access: u32) -> Result<Process, String> {
    // SAFETY: plain call; a null handle is checked.
    let handle = unsafe { OpenProcess(access, 0, pid) };
    if handle.is_null() {
        Err(format!("cannot open process {pid}"))
    } else {
        Ok(Process(handle))
    }
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

/// The process ids of every running `cs2.exe`.
pub fn cs2_processes() -> Vec<u32> {
    let mut out = Vec::new();
    // SAFETY: a snapshot of the process list, walked with a correctly sized entry, closed after.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() || snapshot as isize == -1 {
            return out;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            if wide_to_string(&entry.szExeFile).eq_ignore_ascii_case("cs2.exe") {
                out.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    out
}

/// A process's command line, as the process itself sees it.
pub fn command_line(pid: u32) -> Result<String, String> {
    let process = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    let mut buffer = vec![0u64; 8192]; // 64 KiB, 8-aligned for the UNICODE_STRING at its start
    let mut length = 0u32;
    // SAFETY: the buffer is large enough for a UNICODE_STRING and its text; NtQueryInformationProcess fills it.
    let status = unsafe {
        NtQueryInformationProcess(
            process.0,
            PROCESS_COMMAND_LINE_INFORMATION,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut length,
        )
    };
    if status < 0 {
        return Err(format!(
            "cannot read the command line of process {pid} ({status:#x})"
        ));
    }
    // UNICODE_STRING { u16 Length (bytes), u16 MaximumLength, padding, *u16 Buffer } pointing into the buffer.
    let bytes = (buffer[0] & 0xFFFF) as usize;
    let text = buffer[1] as *const u16;
    let start = buffer.as_ptr() as usize;
    let end = start + buffer.len() * 8;
    if (text as usize) < start || text as usize + bytes > end {
        return Err("the command line is not where Windows said".into());
    }
    // SAFETY: inside our own buffer (checked just above).
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(text, bytes / 2)
    }))
}

/// The file names of the modules a process has loaded.
pub fn modules(pid: u32) -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: a snapshot of the process's modules, walked with a correctly sized entry, closed after.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snapshot.is_null() || snapshot as isize == -1 {
            return out;
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut more = Module32FirstW(snapshot, &mut entry) != 0;
        while more {
            out.push(wide_to_string(&entry.szModule));
            more = Module32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    out
}

/// Has process `pid` load the DLL at `dll` (`LoadLibraryW` on a thread of its own), and waits for it.
fn inject(pid: u32, dll: &Path) -> Result<(), String> {
    let process = open(
        pid,
        PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_WRITE
            | PROCESS_VM_READ,
    )?;
    let path: Vec<u16> = dll
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let size = path.len() * 2;
    // SAFETY: memory allocated in the target for the path, written, freed after its thread ended; LoadLibraryW's
    // address is kernel32's, the same in every process of this boot.
    unsafe {
        let remote = VirtualAllocEx(
            process.0,
            std::ptr::null(),
            size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            return Err("cannot allocate memory in CS2".into());
        }
        let free = |p: *mut c_void| VirtualFreeEx(process.0, p, 0, MEM_RELEASE);
        if WriteProcessMemory(
            process.0,
            remote,
            path.as_ptr().cast(),
            size,
            std::ptr::null_mut(),
        ) == 0
        {
            free(remote);
            return Err("cannot write to CS2's memory".into());
        }
        let kernel32 = GetModuleHandleW(windows_sys::w!("kernel32.dll"));
        let Some(load_library) = GetProcAddress(kernel32, c"LoadLibraryW".as_ptr().cast()) else {
            free(remote);
            return Err("no LoadLibraryW".into());
        };
        let thread = CreateRemoteThread(
            process.0,
            std::ptr::null(),
            0,
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(*mut c_void) -> u32,
            >(load_library)),
            remote,
            0,
            std::ptr::null_mut(),
        );
        if thread.is_null() {
            free(remote);
            return Err("cannot start a thread in CS2".into());
        }
        if WaitForSingleObject(thread, 30_000) != WAIT_OBJECT_0 {
            // LoadLibraryW may still read the path: its memory is left to CS2 rather than freed under it.
            CloseHandle(thread);
            return Err("CS2 did not finish loading OFAST within 30 s".into());
        }
        let mut code = 0u32;
        GetExitCodeThread(thread, &mut code);
        CloseHandle(thread);
        free(remote);
        if code == 0 {
            return Err(format!(
                "CS2 did not load {} (refused by its -insecure check, or not loadable: see %LOCALAPPDATA%\\OFAST\\capture.log)",
                dll.display()
            ));
        }
    }
    Ok(())
}

/// Attaches the DLL built at `dll` to the running CS2 (module docs).
pub fn attach(dll: &Path) -> Result<Attached, String> {
    let pid = match cs2_processes()[..] {
        [pid] => pid,
        [] => return Err("CS2 is not running: start it through Steam with -insecure".into()),
        _ => return Err("several cs2.exe are running".into()),
    };
    let command_line = command_line(pid)?;
    if !gate::is_insecure(&command_line) {
        return Err(format!(
            "CS2 was not started with -insecure: OFAST does not attach to it ({command_line})"
        ));
    }
    if modules(pid)
        .iter()
        .any(|m| m.to_ascii_lowercase().starts_with("ofast_capture"))
    {
        return Err("OFAST is in this CS2 already: restart CS2 to attach again".into());
    }
    if !dll.is_file() {
        return Err(format!("{} not found", dll.display()));
    }
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    let folder = base.join("OFAST").join("capture");
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let copy = folder.join(format!("ofast_capture-{stamp}.dll"));
    std::fs::copy(dll, &copy).map_err(|e| format!("{}: {e}", copy.display()))?;
    inject(pid, &copy)?;
    Ok(Attached {
        pid,
        command_line,
        dll: copy,
    })
}
