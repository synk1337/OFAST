//! The DLL's log, `%LOCALAPPDATA%\OFAST\capture.log`: what it checked, what it hooked, what it saw. Each attach starts
//! it again.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

static LOG: Mutex<Option<(File, Instant)>> = Mutex::new(None);

/// The log file's path.
pub fn path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
    base.join("OFAST").join("capture.log")
}

/// Starts the log (the file emptied).
pub fn open() {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(file) = File::create(&path) {
        let mut log = LOG.lock().unwrap_or_else(PoisonError::into_inner);
        *log = Some((file, Instant::now()));
    }
}

/// One line, after the seconds since the log started.
pub fn line(message: &str) {
    if let Some((file, started)) = LOG.lock().unwrap_or_else(PoisonError::into_inner).as_mut() {
        let _ = writeln!(file, "{:9.3} {message}", started.elapsed().as_secs_f64());
        let _ = file.flush();
    }
}
