//! Byte-level primitives: LEB128 varints, length-prefixed strings, cursor reader.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    Eof,
    BadVarint,
    BadUtf8,
    BadMagic,
    Unsupported(u16),
    Corrupt(&'static str),
    Zstd(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Eof => write!(f, "unexpected end of data"),
            Error::BadVarint => write!(f, "malformed varint"),
            Error::BadUtf8 => write!(f, "invalid utf-8"),
            Error::BadMagic => write!(f, "not a COQD file"),
            Error::Unsupported(v) => write!(f, "unsupported format version {v}"),
            Error::Corrupt(m) => write!(f, "corrupt file: {m}"),
            Error::Zstd(m) => write!(f, "zstd: {m}"),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
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
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    pub fn uvarint(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.buf.push((v as u8) | 0x80);
            v >>= 7;
        }
        self.buf.push(v as u8);
    }
    pub fn str(&mut self, s: &str) {
        self.uvarint(s.len() as u64);
        self.buf.extend_from_slice(s.as_bytes());
    }
    /// 0 = None, otherwise len+1.
    pub fn opt_str(&mut self, s: Option<&str>) {
        match s {
            None => self.uvarint(0),
            Some(s) => {
                self.uvarint(s.len() as u64 + 1);
                self.buf.extend_from_slice(s.as_bytes());
            }
        }
    }
    /// 0 = None, otherwise v+1.
    pub fn opt_id(&mut self, v: Option<u32>) {
        self.uvarint(v.map_or(0, |x| x as u64 + 1));
    }
}

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::Eof)?;
        if end > self.data.len() {
            return Err(Error::Eof);
        }
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn uvarint(&mut self) -> Result<u64> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = self.u8()?;
            if shift >= 64 {
                return Err(Error::BadVarint);
            }
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    pub fn uvarint32(&mut self) -> Result<u32> {
        u32::try_from(self.uvarint()?).map_err(|_| Error::BadVarint)
    }
    pub fn string(&mut self) -> Result<String> {
        let n = self.uvarint()? as usize;
        let b = self.take(n)?;
        String::from_utf8(b.to_vec()).map_err(|_| Error::BadUtf8)
    }
    pub fn opt_string(&mut self) -> Result<Option<String>> {
        let n = self.uvarint()? as usize;
        if n == 0 {
            return Ok(None);
        }
        let b = self.take(n - 1)?;
        Ok(Some(String::from_utf8(b.to_vec()).map_err(|_| Error::BadUtf8)?))
    }
    pub fn opt_id(&mut self) -> Result<Option<u32>> {
        let v = self.uvarint()?;
        if v == 0 {
            Ok(None)
        } else {
            u32::try_from(v - 1).map(Some).map_err(|_| Error::BadVarint)
        }
    }
    pub fn uuid(&mut self) -> Result<[u8; 16]> {
        Ok(self.take(16)?.try_into().unwrap())
    }
}
