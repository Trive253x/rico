//! Opus encoder and decoder.
//!
//! This is the only module that touches libopus (through the `opus` crate,
//! which wraps the C library with FFI). Everything else uses [`Encoder`] and
//! [`Decoder`], so switching to a pure-Rust codec such as `opus-rs` once it
//! is mature only means changing this file.

use anyhow::{bail, Context, Result};

use super::{CHANNELS, FRAME_LEN, SAMPLE_RATE};

/// Largest Opus packet (RFC 6716).
const MAX_PACKET: usize = 1275;
/// Longest frame a packet can hold: 120 ms, interleaved stereo.
const MAX_DECODED: usize = 5760 * CHANNELS;
/// Bitrate of our voice (bits per second), the same as Discord's default.
const BITRATE: i32 = 64_000;

pub struct Encoder(opus::Encoder);

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut enc = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Stereo, opus::Application::Voip)
            .context("creating the Opus encoder")?;
        enc.set_bitrate(opus::Bitrate::Bits(BITRATE))?;
        // In-band FEC lets the receiver rebuild a lost packet from the next one.
        enc.set_inband_fec(true)?;
        enc.set_packet_loss_perc(10)?;
        Ok(Self(enc))
    }

    /// Encodes one 20 ms frame of interleaved stereo samples.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>> {
        if pcm.len() != FRAME_LEN {
            bail!("an Opus frame needs {FRAME_LEN} samples, got {}", pcm.len());
        }
        Ok(self.0.encode_vec_float(pcm, MAX_PACKET)?)
    }
}

pub struct Decoder(opus::Decoder);

impl Decoder {
    pub fn new() -> Result<Self> {
        let dec =
            opus::Decoder::new(SAMPLE_RATE, opus::Channels::Stereo).context("creating the Opus decoder")?;
        Ok(Self(dec))
    }

    /// Decodes a packet into interleaved stereo samples. Malformed packets are
    /// an error, never a panic.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<f32>> {
        if packet.is_empty() || packet.len() > MAX_PACKET {
            bail!("Opus packet of {} bytes", packet.len());
        }
        let mut out = vec![0f32; MAX_DECODED];
        let samples = self.0.decode_float(packet, &mut out, false)?;
        out.truncate(samples * CHANNELS);
        Ok(out)
    }

    /// Packet loss concealment: a plausible 20 ms frame in place of a lost one.
    pub fn conceal(&mut self) -> Result<Vec<f32>> {
        let mut out = vec![0f32; FRAME_LEN];
        let samples = self.0.decode_float(&[], &mut out, false)?;
        out.truncate(samples * CHANNELS);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::FRAME_SAMPLES;

    fn tone(freq: f32, frame: usize) -> Vec<f32> {
        (0..FRAME_SAMPLES)
            .flat_map(|i| {
                let t = (frame * FRAME_SAMPLES + i) as f32 / SAMPLE_RATE as f32;
                let s = (t * freq * std::f32::consts::TAU).sin() * 0.4;
                [s, s]
            })
            .collect()
    }

    fn energy(pcm: &[f32]) -> f32 {
        (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt()
    }

    #[test]
    fn encode_and_decode_round_trip() {
        let mut enc = Encoder::new().unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut last = Vec::new();
        for frame in 0..10 {
            let packet = enc.encode(&tone(440.0, frame)).unwrap();
            assert!(!packet.is_empty() && packet.len() <= MAX_PACKET);
            last = dec.decode(&packet).unwrap();
            assert_eq!(last.len(), FRAME_LEN);
        }
        // After the codec settles, the tone comes back with similar energy.
        let (input, output) = (energy(&tone(440.0, 9)), energy(&last));
        assert!((output - input).abs() < input * 0.5, "in {input}, out {output}");
        assert_eq!(dec.conceal().unwrap().len(), FRAME_LEN);
    }

    #[test]
    fn wrong_input_is_an_error() {
        let mut enc = Encoder::new().unwrap();
        assert!(enc.encode(&[0.0; 100]).is_err());
        let mut dec = Decoder::new().unwrap();
        assert!(dec.decode(&[]).is_err());
        assert!(dec.decode(&[0u8; 2000]).is_err());
        // Random bytes: an error or some decoded audio, but never a crash.
        let garbage: Vec<u8> = (0..200u32).map(|i| (i * 37 % 251) as u8).collect();
        let _ = dec.decode(&garbage);
    }
}
