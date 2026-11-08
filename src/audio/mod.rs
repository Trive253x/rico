//! Voice audio: Opus codec, sample processing and audio devices.
//!
//! Discord voice is Opus at 48 kHz, stereo, 20 ms per packet. Inside ferrocord
//! audio is `f32` samples in that format; conversion to and from the devices'
//! own rate and channel count happens here, outside the device callbacks, so
//! that the callbacks only copy samples.

pub mod codec;
pub mod device;
pub mod dsp;
pub mod jitter;

/// Sample rate used by Discord voice.
pub const SAMPLE_RATE: u32 = 48_000;
/// Channels of the Opus stream (stereo).
pub const CHANNELS: usize = 2;
/// Samples per channel in one 20 ms frame.
pub const FRAME_SAMPLES: usize = 960;
/// Interleaved samples in one 20 ms stereo frame.
pub const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS;
