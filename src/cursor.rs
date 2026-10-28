//! Low-level binary reader: little-endian, no `unsafe`, no overflows.
//!
//! `Cursor` works over an already loaded `&[u8]` (whole file or decompressed
//! payload), so reads are **zero-copy**: nothing is allocated on the heap
//! unless the caller explicitly asks for it.

use crate::error::RoflError;

#[derive(Debug, Clone)]
pub struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    #[inline]
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Absolute position within the buffer.
    #[inline]
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Remaining bytes (invariant: `pos <= buf.len()`).
    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    /// Consumes exactly `n` bytes.
    #[inline]
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], RoflError> {
        let end = self.pos.checked_add(n).ok_or(RoflError::OutOfBounds {
            offset: self.pos,
            needed: n,
            remaining: self.remaining(),
        })?;
        if end > self.buf.len() {
            return Err(RoflError::OutOfBounds {
                offset: self.pos,
                needed: n,
                remaining: self.remaining(),
            });
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<(), RoflError> {
        self.read_bytes(n).map(|_| ())
    }

    /// Seeks to a validated absolute position.
    #[inline]
    pub fn seek_to(&mut self, pos: usize) -> Result<(), RoflError> {
        if pos > self.buf.len() {
            return Err(RoflError::OutOfBounds {
                offset: self.pos,
                needed: pos - self.pos,
                remaining: self.remaining(),
            });
        }
        self.pos = pos;
        Ok(())
    }

    #[inline]
    pub fn read_u8(&mut self) -> Result<u8, RoflError> {
        Ok(self.read_bytes(1)?[0])
    }

    #[inline]
    pub fn read_u16_le(&mut self) -> Result<u16, RoflError> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    #[inline]
    pub fn read_u32_le(&mut self) -> Result<u32, RoflError> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    #[inline]
    pub fn read_u64_le(&mut self) -> Result<u64, RoflError> {
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    #[inline]
    pub fn read_f32_le(&mut self) -> Result<f32, RoflError> {
        Ok(f32::from_bits(self.read_u32_le()?))
    }

    /// Reads a string using the "7-bit encoded int" length encoding used by
    /// .NET (`BinaryReader.ReadString`): a length prefix in bytes where each
    /// byte contributes 7 bits (bit 7 = "continues").
    ///
    /// UTF-8 and a reasonable maximum length are validated; in any other case
    /// an error is returned so the caller can degrade gracefully.
    pub fn read_dotnet_string(&mut self) -> Result<&'a str, RoflError> {
        let mut len: usize = 0;
        let mut shift = 0u32;
        for _ in 0..5 {
            let b = self.read_u8()?;
            len |= ((b & 0x7F) as usize) << shift;
            if b & 0x80 == 0 {
                let bytes = self.read_bytes(len)?;
                return std::str::from_utf8(bytes).map_err(|_| RoflError::Payload {
                    offset: self.pos,
                    reason: format!("cadena de {len} bytes no es UTF-8 válido"),
                });
            }
            shift += 7;
        }
        Err(RoflError::OutOfBounds {
            offset: self.pos,
            needed: 1,
            remaining: self.remaining(),
        })
    }
}
