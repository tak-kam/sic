//! Reading and writing the little-endian binary encoding shared by the file
//! formats.
//!
//! Both the bytecode format and the checkpoint format need the same handful of
//! primitives, and both need reading to be safe against a file that is hostile
//! rather than merely truncated. Every read is bounds-checked and every
//! variable-length item is preceded by its length.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinError {
    pub message: String,
}

impl BinError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for BinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type Result<T> = std::result::Result<T, BinError>;

#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.buf.push(v as u8);
    }

    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn u128(&mut self, v: u128) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }

    pub fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }

    pub fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.buf.extend_from_slice(s.as_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    pub fn at_end(&self) -> bool {
        self.pos == self.bytes.len()
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| BinError::new("length overflows"))?;
        if end > self.bytes.len() {
            return Err(BinError::new("unexpected end of input"));
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(BinError::new(format!("{other} is not a boolean"))),
        }
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        let mut out = [0u8; 8];
        out.copy_from_slice(b);
        Ok(u64::from_le_bytes(out))
    }

    pub fn u128(&mut self) -> Result<u128> {
        let b = self.take(16)?;
        let mut out = [0u8; 16];
        out.copy_from_slice(b);
        Ok(u128::from_le_bytes(out))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(self.u64()? as i64)
    }

    pub fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_bits(self.u64()?))
    }

    pub fn str(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|_| BinError::new("a string in the file is not valid UTF-8"))
    }

    /// Reads an element count, refusing one that cannot fit in what is left.
    ///
    /// Without this a corrupt file could ask for an enormous allocation before
    /// anything else notices it is corrupt.
    pub fn count(&mut self, bytes_per_element: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        let minimum = n.saturating_mul(bytes_per_element.max(1));
        if minimum > self.remaining() {
            return Err(BinError::new(
                "an element count is larger than the data holding it",
            ));
        }
        Ok(n)
    }

    pub fn expect_end(&self, what: &str) -> Result<()> {
        if !self.at_end() {
            return Err(BinError::new(format!(
                "{} bytes left over at the end of {what}",
                self.remaining()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_primitive() {
        let mut w = Writer::new();
        w.u8(1);
        w.bool(true);
        w.u16(2);
        w.u32(3);
        w.u64(4);
        w.u128(5);
        w.i64(-6);
        w.f64(1.5);
        w.str("hi");
        let bytes = w.finish();

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 1);
        assert!(r.bool().unwrap());
        assert_eq!(r.u16().unwrap(), 2);
        assert_eq!(r.u32().unwrap(), 3);
        assert_eq!(r.u64().unwrap(), 4);
        assert_eq!(r.u128().unwrap(), 5);
        assert_eq!(r.i64().unwrap(), -6);
        assert_eq!(r.f64().unwrap(), 1.5);
        assert_eq!(r.str().unwrap(), "hi");
        assert!(r.at_end());
    }

    #[test]
    fn reading_past_the_end_is_an_error_not_a_panic() {
        let bytes = [1u8, 2];
        let mut r = Reader::new(&bytes);
        assert!(r.u32().is_err());
        let mut r = Reader::new(&bytes);
        assert!(r.str().is_err());
    }

    #[test]
    fn a_huge_count_is_refused_before_allocating() {
        let mut w = Writer::new();
        w.u32(u32::MAX);
        let bytes = w.finish();
        let mut r = Reader::new(&bytes);
        assert!(r.count(8).is_err());
    }

    #[test]
    fn a_non_boolean_byte_is_rejected() {
        let bytes = [7u8];
        assert!(Reader::new(&bytes).bool().is_err());
    }
}
