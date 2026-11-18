//! Video of Go Live streams: H.264 over RTP in, pictures out; and our own
//! stream: captured pictures in, H.264 over RTP out.
//!
//! - [`h264`]: puts the RTP packets of a frame back together (RFC 6184) into
//!   an Annex B frame, the form DAVE decrypts and the decoder reads, and
//!   splits our frames into packets;
//! - [`decoder`]: decodes on its own thread with `OpenH264` (C++ through FFI,
//!   only here and in `encoder`): the input comes from other people, so it
//!   gets size limits, a bounded queue and a panic guard;
//! - [`convert`]: scales captured pictures and converts them to I420 (pure
//!   Rust);
//! - [`encoder`]: encodes our stream on its own thread with `OpenH264`, the
//!   same library as the decoder, with a one-place input and a bounded
//!   output;
//! - [`yuv`]: decoded pictures to RGBA with the right colours (BT.709 for
//!   HD) and smooth colour edges;
//! - [`mp4`]: reads the short MP4 clips Discord's media proxy serves for
//!   Tenor and Giphy GIFs, decoded with the same decoder (see `media`).

pub mod convert;
pub mod decoder;
pub mod encoder;
pub mod h264;
pub mod mp4;
pub mod yuv;

/// Largest picture accepted from a stream (4K).
pub const MAX_WIDTH: usize = 3840;
pub const MAX_HEIGHT: usize = 2160;

/// A decoded picture, RGBA.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
