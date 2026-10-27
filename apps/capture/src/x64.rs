//! The little x64 machine code ofast-capture reads and writes in CS2 (decision 022): a jump thunk's target, where a
//! function stores its second argument, byte patterns with wildcards, and the view setup's 16-byte patch.

/// Where a thunk at `address` jumps to, if its code starts with a `jmp rel32` (`E9 xx xx xx xx`).
pub fn jump_target(code: &[u8], address: usize) -> Option<usize> {
    match code {
        [0xE9, a, b, c, d, ..] => {
            let rel = i32::from_le_bytes([*a, *b, *c, *d]);
            Some((address as i64 + 5 + i64::from(rel)) as usize)
        }
        _ => None,
    }
}

/// The address a function at `address` stores its second argument to (`rdx`, x64's) with
/// `mov qword ptr [rip + disp32], rdx` (`48 89 15 disp32`), the first such store in `code` before the function's end
/// (its first `ret` or `int3` byte): how a setter keeps a pointer in a global (CS2's `SetGlobals`).
pub fn rdx_store(code: &[u8], address: usize) -> Option<usize> {
    let end = code
        .iter()
        .position(|&b| b == 0xC3 || b == 0xCC)
        .unwrap_or(code.len());
    code[..end]
        .windows(7)
        .enumerate()
        .find_map(|(at, w)| match w {
            [0x48, 0x89, 0x15, a, b, c, d] => {
                let disp = i32::from_le_bytes([*a, *b, *c, *d]);
                Some((address as i64 + at as i64 + 7 + i64::from(disp)) as usize)
            }
            _ => None,
        })
}

/// A byte pattern: `Some(byte)` must match, `None` matches anything.
pub type Pattern = Vec<Option<u8>>;

/// A pattern written as hex bytes with `??` for any byte (`"48 8b 0d ?? ?? ?? ??"`); `None` for a malformed one.
pub fn pattern(text: &str) -> Option<Pattern> {
    text.split_whitespace()
        .map(|b| {
            if b == "??" {
                Some(None)
            } else {
                u8::from_str_radix(b, 16).ok().map(Some)
            }
        })
        .collect()
}

/// Every offset in `code` where `pattern` matches.
pub fn find(code: &[u8], pattern: &[Option<u8>]) -> Vec<usize> {
    if pattern.is_empty() || code.len() < pattern.len() {
        return Vec::new();
    }
    code.windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| {
            w.iter()
                .zip(pattern)
                .all(|(b, p)| p.is_none_or(|p| p == *b))
        })
        .map(|(at, _)| at)
        .collect()
}

/// `mov rcx, rbx; mov rax, function; call rax; nop`: 16 bytes calling `function` with `rbx` as its first argument
/// (the view setup's patch, decision 022).
pub fn call_with_rbx(function: u64) -> [u8; 16] {
    let mut code = [0u8; 16];
    code[..3].copy_from_slice(&[0x48, 0x89, 0xD9]);
    code[3..5].copy_from_slice(&[0x48, 0xB8]);
    code[5..13].copy_from_slice(&function.to_le_bytes());
    code[13..].copy_from_slice(&[0xFF, 0xD0, 0x90]);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_match_with_wildcards() {
        let p = pattern("48 8b ?? 01").unwrap();
        assert_eq!(p, vec![Some(0x48), Some(0x8b), None, Some(0x01)]);
        let code = [0x00, 0x48, 0x8b, 0x77, 0x01, 0x48, 0x8b, 0x00, 0x02];
        assert_eq!(find(&code, &p), vec![1]);
        assert!(pattern("48 zz").is_none());
        assert!(find(&code[..2], &p).is_empty());
    }

    #[test]
    fn the_patch_is_sixteen_bytes() {
        let code = call_with_rbx(0x1122_3344_5566_7788);
        assert_eq!(&code[..5], &[0x48, 0x89, 0xD9, 0x48, 0xB8]);
        assert_eq!(&code[5..13], &0x1122_3344_5566_7788u64.to_le_bytes());
        assert_eq!(&code[13..], &[0xFF, 0xD0, 0x90]);
    }

    #[test]
    fn jumps_are_relative_to_the_next_instruction() {
        assert_eq!(
            jump_target(&[0xE9, 0x10, 0, 0, 0, 0xCC], 0x1000),
            Some(0x1015)
        );
        assert_eq!(
            jump_target(&[0xE9, 0xFB, 0xFF, 0xFF, 0xFF], 0x1000),
            Some(0x1000)
        );
        assert_eq!(jump_target(&[0x48, 0x89], 0x1000), None);
    }

    #[test]
    fn a_setter_stores_rdx_rip_relative() {
        // mov rax, rcx (3 bytes); mov [rip+0x100], rdx; ret
        let code = [
            0x48, 0x89, 0xC8, 0x48, 0x89, 0x15, 0x00, 0x01, 0x00, 0x00, 0xC3,
        ];
        assert_eq!(rdx_store(&code, 0x2000), Some(0x2000 + 3 + 7 + 0x100));
        // mov [rip+x], rcx is not rdx.
        assert_eq!(rdx_store(&[0x48, 0x89, 0x0D, 0, 0, 0, 0], 0), None);
        // A store after the function's ret belongs to the next function.
        let next = [0xC3, 0xCC, 0x48, 0x89, 0x15, 0, 0, 0, 0];
        assert_eq!(rdx_store(&next, 0), None);
    }
}
