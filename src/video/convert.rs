//! Captured pictures (BGRA or RGBA rows, as Windows gives them) scaled and
//! converted to I420 for the encoder, plus a small RGBA copy for the preview.
//! Pure Rust, every index checked by the slice bounds.

use super::encoder::Yuv;
use super::Picture;

/// Order of the bytes of a captured pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    Bgra,
    Rgba,
}

/// A captured picture: `height` rows of `pitch` bytes, 4 bytes per pixel.
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub width: usize,
    pub height: usize,
    pub pitch: usize,
    pub order: PixelOrder,
}

impl Frame<'_> {
    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.pitch >= self.width * 4
            && self.data.len() >= self.pitch * (self.height - 1) + self.width * 4
    }

    /// (r, g, b) of the pixel at (x, y), clamped inside the picture.
    #[inline]
    fn rgb(&self, x: usize, y: usize) -> (u32, u32, u32) {
        let x = x.min(self.width - 1);
        let y = y.min(self.height - 1);
        let i = y * self.pitch + x * 4;
        match self.data.get(i..i + 3) {
            Some(&[a, b, c]) => match self.order {
                PixelOrder::Bgra => (u32::from(c), u32::from(b), u32::from(a)),
                PixelOrder::Rgba => (u32::from(a), u32::from(b), u32::from(c)),
            },
            _ => (0, 0, 0),
        }
    }

    /// Average colour of the source area under the output pixel (ox, oy),
    /// from up to 2×2 samples (enough to keep text readable when shrinking).
    #[inline]
    fn sample(&self, ox: usize, oy: usize, sx: u64, sy: u64) -> (u32, u32, u32) {
        // Source coordinates in 16.16 fixed point.
        let x0 = (ox as u64 * sx) >> 16;
        let y0 = (oy as u64 * sy) >> 16;
        let x1 = ((ox as u64 * sx + sx / 2) >> 16).max(x0);
        let y1 = ((oy as u64 * sy + sy / 2) >> 16).max(y0);
        let (x0, y0, x1, y1) = (x0 as usize, y0 as usize, x1 as usize, y1 as usize);
        let a = self.rgb(x0, y0);
        let b = self.rgb(x1, y0);
        let c = self.rgb(x0, y1);
        let d = self.rgb(x1, y1);
        (
            (a.0 + b.0 + c.0 + d.0) / 4,
            (a.1 + b.1 + c.1 + d.1) / 4,
            (a.2 + b.2 + c.2 + d.2) / 4,
        )
    }
}

/// Size of the picture sent: fitted inside `max` keeping the proportions,
/// never enlarged, even sides (as H.264 4:2:0 wants), at least 16×16.
pub fn fit(width: usize, height: usize, max: (u32, u32)) -> (usize, usize) {
    let (mw, mh) = (max.0 as usize, max.1 as usize);
    if width == 0 || height == 0 {
        return (16, 16);
    }
    let scale = (mw as f64 / width as f64).min(mh as f64 / height as f64).min(1.0);
    let w = ((width as f64 * scale) as usize).max(16) & !1;
    let h = ((height as f64 * scale) as usize).max(16) & !1;
    (w.min(super::MAX_WIDTH), h.min(super::MAX_HEIGHT))
}

/// BT.601 limited range, as decoders assume without other signs.
#[inline]
fn luma(r: u32, g: u32, b: u32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16).min(255) as u8
}

#[inline]
fn chroma(r: u32, g: u32, b: u32) -> (u8, u8) {
    let (r, g, b) = (r as i32, g as i32, b as i32);
    let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
    let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
    (u.clamp(0, 255) as u8, v.clamp(0, 255) as u8)
}

/// The frame scaled to `size` (even sides) and converted to I420. `None` if
/// the frame or the size are not valid.
pub fn to_i420(frame: &Frame<'_>, size: (usize, usize)) -> Option<Yuv> {
    let (w, h) = size;
    if !frame.valid() || w == 0 || h == 0 || w % 2 == 1 || h % 2 == 1 {
        return None;
    }
    let sx = ((frame.width as u64) << 16) / w as u64;
    let sy = ((frame.height as u64) << 16) / h as u64;
    let mut data = vec![0u8; w * h * 3 / 2];
    let (y_plane, uv) = data.split_at_mut(w * h);
    let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
    for oy in (0..h).step_by(2) {
        for ox in (0..w).step_by(2) {
            // Four luma samples, their average for the chroma.
            let mut sum = (0, 0, 0);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (r, g, b) = frame.sample(ox + dx, oy + dy, sx, sy);
                y_plane[(oy + dy) * w + ox + dx] = luma(r, g, b);
                sum = (sum.0 + r, sum.1 + g, sum.2 + b);
            }
            let (u, v) = chroma(sum.0 / 4, sum.1 / 4, sum.2 / 4);
            let ci = (oy / 2) * (w / 2) + ox / 2;
            u_plane[ci] = u;
            v_plane[ci] = v;
        }
    }
    Yuv::new(w, h, data)
}

/// A small RGBA copy for the preview of what we send (at most `max_width`
/// wide).
pub fn preview(frame: &Frame<'_>, max_width: usize) -> Option<Picture> {
    if !frame.valid() || max_width == 0 {
        return None;
    }
    let (w, h) = fit(
        frame.width,
        frame.height,
        (max_width as u32, super::MAX_HEIGHT as u32),
    );
    let sx = ((frame.width as u64) << 16) / w as u64;
    let sy = ((frame.height as u64) << 16) / h as u64;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for oy in 0..h {
        for ox in 0..w {
            let (r, g, b) = frame.sample(ox, oy, sx, sy);
            rgba.extend_from_slice(&[r as u8, g as u8, b as u8, 255]);
        }
    }
    Some(Picture {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, pitch: usize, bgra: [u8; 4]) -> Vec<u8> {
        let mut data = vec![0u8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                data[y * pitch + x * 4..y * pitch + x * 4 + 4].copy_from_slice(&bgra);
            }
        }
        data
    }

    #[test]
    fn sizes_fit_and_stay_even() {
        assert_eq!(fit(2560, 1440, (1280, 720)), (1280, 720));
        assert_eq!(fit(1920, 1200, (1280, 720)), (1152, 720));
        // Never enlarged.
        assert_eq!(fit(801, 601, (1280, 720)), (800, 600));
        assert_eq!(fit(4, 4, (1280, 720)), (16, 16));
        assert_eq!(fit(0, 10, (1280, 720)), (16, 16));
    }

    #[test]
    fn colours_survive_scaling_and_padding() {
        // A red picture with padding at the end of each row (as Windows does).
        let (w, h, pitch) = (64, 36, 64 * 4 + 32);
        let data = solid(w, h, pitch, [0, 0, 255, 255]);
        let frame = Frame {
            data: &data,
            width: w,
            height: h,
            pitch,
            order: PixelOrder::Bgra,
        };
        let yuv = to_i420(&frame, (32, 18)).unwrap();
        assert_eq!(yuv.data.len(), 32 * 18 * 3 / 2);
        // Red in BT.601: Y 81, U 90, V 240.
        assert!(yuv.data[..32 * 18].iter().all(|&y| (80..=83).contains(&y)));
        let u = yuv.data[32 * 18];
        let v = yuv.data[32 * 18 + 32 * 18 / 4];
        assert!((88..=92).contains(&u) && (238..=241).contains(&v), "{u} {v}");
        // The same bytes read as RGBA are blue.
        let frame = Frame {
            order: PixelOrder::Rgba,
            ..frame
        };
        let p = preview(&frame, 32).unwrap();
        assert_eq!((p.width, p.height), (32, 18));
        assert_eq!(&p.rgba[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn invalid_frames_are_refused() {
        let data = vec![0u8; 100];
        let frame = Frame {
            data: &data,
            width: 64,
            height: 64,
            pitch: 256,
            order: PixelOrder::Bgra,
        };
        assert!(to_i420(&frame, (32, 32)).is_none());
        assert!(preview(&frame, 32).is_none());
        let small = Frame {
            data: &data,
            width: 5,
            height: 5,
            pitch: 20,
            order: PixelOrder::Bgra,
        };
        assert!(to_i420(&small, (3, 4)).is_none());
        assert!(to_i420(&small, (4, 4)).is_some());
    }
}
