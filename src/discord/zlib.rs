//! Decompressione del trasporto `zlib-stream` del gateway.
//!
//! Il server comprime tutta la connessione come un unico stream zlib e chiude
//! ogni messaggio con un flush "sync" (suffisso `00 00 ff ff`). Il contesto
//! di decompressione va mantenuto per tutta la vita della connessione.

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

    /// Aggiunge un frame binario; restituisce un messaggio JSON quando è completo.
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

            // Finito quando l'input è consumato e l'inflater non ha più nulla
            // in sospeso (buffer non pieno, oppure un giro senza output).
            if input.is_empty() && (self.out.len() < self.out.capacity() || produced == 0) {
                break;
            }
            if consumed == 0 && produced == 0 {
                bail!("stream zlib bloccato");
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
    fn decomprime_piu_messaggi_a_pezzi() {
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

        // Il secondo messaggio arriva diviso in più frame.
        let (a, b) = second.split_at(second.len() / 2);
        assert_eq!(z.push(a).unwrap(), None);
        assert_eq!(z.push(b).unwrap().as_deref(), Some(big.as_str()));
    }
}
