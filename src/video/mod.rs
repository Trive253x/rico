//! Video of Go Live streams: H.264 over RTP in, pictures out.
//!
//! - [`h264`]: puts the RTP packets of a frame back together (RFC 6184) into
//!   an Annex B frame, the form DAVE decrypts and the decoder reads;
//! - [`decoder`]: decodes on its own thread with OpenH264. It is the only
//!   module that touches OpenH264 (C++ through FFI): the input comes from
//!   other people, so it gets size limits, a bounded queue and a panic guard.

pub mod decoder;
pub mod h264;

/// Largest picture accepted from a stream (4K).
pub const MAX_WIDTH: usize = 3840;
pub const MAX_HEIGHT: usize = 2160;

/// A decoded picture, RGBA.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
