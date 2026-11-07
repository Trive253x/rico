//! Decompression of the gateway's `zlib-stream` transport.
//!
//! The server compresses the whole connection as a single zlib stream and
//! ends every message with a "sync" flush (suffix `00 00 ff ff`). The
//! decompression context must live as long as the connection.

use anyhow::{bail, Result};
use flate2::{Decompress, FlushDecompress};

const SUFFIX: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
const CHUNK: usize = 64 * 1024;

pub struct ZlibStream {
    pending: Vec<u8>,
    inflater: Decompress,
    out: Vec<u8>,
}

impl ZlibStream {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            inflater: Decompress::new(true),
            out: Vec::with_capacity(CHUNK),
        }
    }

    /// Adds a binary frame; returns a JSON message once it is complete.
    pub fn push(&mut self, data: &[u8]) -> Result<Option<String>> {
        self.pending.extend_from_slice(data);
        if !self.pending.ends_with(&SUFFIX) {
            return Ok(None);
        }

        self.out.clear();
        let mut input: &[u8] = &self.pending;
        loop {
            if self.out.len() == self.out.capacity() {
                self.out.reserve(CHUNK);
            }
            let (in_before, out_before) = (self.inflater.total_in(), self.inflater.total_out());
            self.inflater
                .decompress_vec(input, &mut self.out, FlushDecompress::Sync)?;
            let consumed = (self.inflater.total_in() - in_before) as usize;
            let produced = self.inflater.total_out() - out_before;
            input = &input[consumed..];

            // Done when the input is consumed and the inflater has nothing
            // pending (buffer not full, or a round without output).
            if input.is_empty() && (self.out.len() < self.out.capacity() || produced == 0) {
                break;
            }
            if consumed == 0 && produced == 0 {
                bail!("zlib stream stuck");
            }
        }
        self.pending.clear();
        Ok(Some(String::from_utf8(self.out.clone())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    #[test]
    fn decompresses_several_messages_in_pieces() {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        let big = format!(r#"{{"op":0,"d":"{}"}}"#, "x".repeat(200_000));

        enc.write_all(br#"{"op":10}"#).unwrap();
        enc.flush().unwrap();
        let first = enc.get_ref().clone();

        enc.write_all(big.as_bytes()).unwrap();
        enc.flush().unwrap();
        let second = enc.get_ref()[first.len()..].to_vec();

        let mut z = ZlibStream::new();
        assert_eq!(z.push(&first).unwrap().as_deref(), Some(r#"{"op":10}"#));

        // The second message arrives split over several frames.
        let (a, b) = second.split_at(second.len() / 2);
        assert_eq!(z.push(a).unwrap(), None);
        assert_eq!(z.push(b).unwrap().as_deref(), Some(big.as_str()));
    }
}
