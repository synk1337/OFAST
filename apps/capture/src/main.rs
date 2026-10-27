//! `ofast-attach`: loads OFAST's capture DLL into a running CS2 started with `-insecure` (refuses any other).
//!
//! ```text
//! ofast-attach                  the DLL next to this program (ofast_capture.dll)
//! ofast-attach path\to\ofast_capture.dll
//! ```
//!
//! The DLL then logs what it does to `%LOCALAPPDATA%\OFAST\capture.log`.

use std::process::ExitCode;

#[cfg(windows)]
fn main() -> ExitCode {
    use std::path::PathBuf;
    let dll = std::env::args_os().nth(1).map(PathBuf::from).or_else(|| {
        std::env::current_exe().ok().and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join(ofast_capture::attach::DLL_NAME))
        })
    });
    let Some(dll) = dll else {
        eprintln!("error: no DLL given");
        return ExitCode::FAILURE;
    };
    match ofast_capture::attach::attach(&dll) {
        Ok(attached) => {
            println!(
                "attached to CS2 (process {}), started with -insecure: {}",
                attached.pid, attached.command_line
            );
            println!("loaded {}", attached.dll.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> ExitCode {
    eprintln!("ofast-attach works on Windows only (CS2's capture)");
    ExitCode::FAILURE
}
