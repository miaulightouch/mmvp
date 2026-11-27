//! Minimal byte reader with switchable endianness.
use anyhow::{bail, Result};

pub struct Rd<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
    pub be: bool,
}

macro_rules! rd_num {
    ($name:ident, $t:ty, $n:expr) => {
        pub fn $name(&mut self) -> Result<$t> {
            let b: [u8; $n] = self.take($n)?.try_into().unwrap();
            Ok(if self.be { <$t>::from_be_bytes(b) } else { <$t>::from_le_bytes(b) })
        }
    };
}

impl<'a> Rd<'a> {
    pub fn new(buf: &'a [u8], be: bool) -> Self {
        Rd { buf, pos: 0, be }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.buf.len() {
            bail!("read past end ({} + {} > {})", self.pos, n, self.buf.len());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    rd_num!(u16, u16, 2);
    rd_num!(i16, i16, 2);
    rd_num!(u32, u32, 4);
    rd_num!(i32, i32, 4);
    rd_num!(u64, u64, 8);
    rd_num!(i64, i64, 8);
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn cstr(&mut self) -> Result<String> {
        let rest = &self.buf[self.pos..];
        let Some(e) = rest.iter().position(|&c| c == 0) else { bail!("unterminated string") };
        let s = String::from_utf8_lossy(&rest[..e]).into_owned();
        self.pos += e + 1;
        Ok(s)
    }
    pub fn align(&mut self, n: usize) {
        self.pos = self.pos.next_multiple_of(n);
    }
}
