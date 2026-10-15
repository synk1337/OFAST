//! A small little-endian byte cursor shared by every binary reader in OFAST.
//!
//! Valve's formats (VPK, compiled resources, binary KeyValues3, demos) are all little-endian and are read from
//! in-memory buffers. [`ByteReader`] walks such a buffer and turns "ran out of bytes" into a descriptive
//! [`UnexpectedEof`] error that says *what* was being read and *where*, which makes corrupt or misunderstood files
//! much easier to debug than a bare index panic.

/// Error returned when a read needs more bytes than the buffer has left.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unexpected end of data while reading {what} at offset {offset} \
     (needed {needed} bytes, {available} available)"
)]
pub struct UnexpectedEof {
    /// Human-readable name of the field being read, e.g. `"VPK header"`.
    pub what: &'static str,
    /// Position in the buffer where the read started.
    pub offset: usize,
    /// Number of bytes the read needed.
    pub needed: usize,
    /// Number of bytes that were actually left.
    pub available: usize,
}

/// Cursor over a byte slice that reads little-endian values.
///
/// Every method takes a `what` label used only for error messages.
#[derive(Debug, Clone)]
pub struct ByteReader<'a> {
    data: &'a [u8],
    pos: usize,
}

/// Generates `fn name(&mut self, what) -> Result<T, UnexpectedEof>` for a little-endian integer/float type.
macro_rules! read_le {
    ($($name:ident => $ty:ty),* $(,)?) => {
        $(
            #[doc = concat!("Reads a little-endian `", stringify!($ty), "`.")]
            pub fn $name(&mut self, what: &'static str) -> Result<$ty, UnexpectedEof> {
                let bytes = self.take(std::mem::size_of::<$ty>(), what)?;
                // `take` returned exactly size_of::<$ty>() bytes, so the conversion cannot fail.
                Ok(<$ty>::from_le_bytes(bytes.try_into().expect("take returned the requested length")))
            }
        )*
    };
}

impl<'a> ByteReader<'a> {
    /// Starts reading `data` at offset 0.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Current offset from the start of the buffer.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Number of bytes left after the current position.
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    /// Moves the cursor to an absolute offset.
    pub fn seek(&mut self, offset: usize, what: &'static str) -> Result<(), UnexpectedEof> {
        if offset > self.data.len() {
            return Err(UnexpectedEof {
                what,
                offset,
                needed: 0,
                available: 0,
            });
        }
        self.pos = offset;
        Ok(())
    }

    /// Returns the next `len` bytes and advances past them.
    pub fn take(&mut self, len: usize, what: &'static str) -> Result<&'a [u8], UnexpectedEof> {
        let available = self.remaining();
        if len > available {
            return Err(UnexpectedEof {
                what,
                offset: self.pos,
                needed: len,
                available,
            });
        }
        let bytes = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok(bytes)
    }

    /// Advances past `len` bytes without looking at them.
    pub fn skip(&mut self, len: usize, what: &'static str) -> Result<(), UnexpectedEof> {
        self.take(len, what).map(|_| ())
    }

    /// Reads a fixed-size byte array, e.g. a 16-byte MD5 hash.
    pub fn array<const N: usize>(&mut self, what: &'static str) -> Result<[u8; N], UnexpectedEof> {
        let bytes = self.take(N, what)?;
        Ok(bytes
            .try_into()
            .expect("take returned the requested length"))
    }

    /// Reads a NUL-terminated string and returns its bytes *without* the terminator.
    pub fn cstr(&mut self, what: &'static str) -> Result<&'a [u8], UnexpectedEof> {
        let rest = &self.data[self.pos..];
        match rest.iter().position(|&b| b == 0) {
            Some(len) => {
                let bytes = &rest[..len];
                self.pos += len + 1;
                Ok(bytes)
            }
            None => Err(UnexpectedEof {
                what,
                offset: self.pos,
                needed: rest.len() + 1,
                available: rest.len(),
            }),
        }
    }

    read_le! {
        u8 => u8,
        u16 => u16,
        u32 => u32,
        u64 => u64,
        i8 => i8,
        i16 => i16,
        i32 => i32,
        i64 => i64,
        f32 => f32,
        f64 => f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_values_in_order() {
        let data = [0x34, 0x12, 0xAA, 0x55, 0x02, 0x00, b'h', b'i', 0x00, 0xFF];
        let mut r = ByteReader::new(&data);
        assert_eq!(r.u32("sig").unwrap(), 0x55AA_1234);
        assert_eq!(r.u16("ver").unwrap(), 2);
        assert_eq!(r.cstr("name").unwrap(), b"hi");
        assert_eq!(r.u8("tail").unwrap(), 0xFF);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn eof_error_says_what_and_where() {
        let mut r = ByteReader::new(&[1, 2, 3]);
        r.skip(2, "padding").unwrap();
        let err = r.u32("entry CRC32").unwrap_err();
        assert_eq!(
            err,
            UnexpectedEof {
                what: "entry CRC32",
                offset: 2,
                needed: 4,
                available: 1
            }
        );
    }

    #[test]
    fn unterminated_string_is_an_error() {
        let mut r = ByteReader::new(b"abc");
        assert!(r.cstr("name").is_err());
        // A failed read must not move the cursor.
        assert_eq!(r.position(), 0);
    }
}
