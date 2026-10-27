//! Hooking by vtable slot: one pointer in an interface's table swapped for OFAST's function, the original kept to
//! call on (decision 022).

use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Memory::{PAGE_PROTECTION_FLAGS, PAGE_READWRITE, VirtualProtect};

/// Puts `function` in slot `index` of `vtable`, the slot's function kept in `original` first: a call landing on the
/// hook the moment the slot changes finds the original there already.
///
/// # Safety
///
/// `vtable` must be a live vtable with at least `index + 1` slots, and `function` a function with the slot's exact
/// signature and calling convention, for the rest of the process's life, that calls on through `original`.
pub unsafe fn swap_slot(
    vtable: *mut usize,
    index: usize,
    function: usize,
    original: &AtomicUsize,
) -> Result<(), String> {
    // SAFETY: the caller's: the slot exists.
    let slot = unsafe { vtable.add(index) };
    let mut old: PAGE_PROTECTION_FLAGS = 0;
    // SAFETY: an 8-byte range inside the vtable; vtables live in read-only data, so it is made writable around the
    // write and put back.
    unsafe {
        original.store(slot.read_volatile(), Ordering::Release);
        if VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old) == 0 {
            return Err(format!("VirtualProtect failed on slot {index}"));
        }
        slot.write_volatile(function);
        let mut ignored: PAGE_PROTECTION_FLAGS = 0;
        VirtualProtect(slot.cast(), 8, old, &mut ignored);
    }
    Ok(())
}
