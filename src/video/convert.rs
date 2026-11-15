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

/// Source positions under each output position along one axis, clamped
/// inside the picture: the start of the area and its middle (the two
/// samples [`Frame::sample`] takes), in 16.16 fixed point as there.
fn taps(out: usize, src: usize) -> Vec<(usize, usize)> {
    let step = ((src as u64) << 16) / out as u64;
    let last = src.saturating_sub(1) as u64;
    (0..out as u64)
        .map(|o| {
            let a = (o * step) >> 16;
            let b = ((o * step + step / 2) >> 16).max(a);
            (a.min(last) as usize, b.min(last) as usize)
        })
        .collect()
}

/// One captured row: exactly `width` pixels (padding left out).
fn source_row<'a>(frame: &Frame<'a>, y: usize) -> &'a [u8] {
    let start = y * frame.pitch;
    frame.data.get(start..start + frame.width * 4).unwrap_or_default()
}

/// (r, g, b) of the pixel at byte `offset` of a row; `BGRA` picks the byte
/// order (a constant, so the choice is made at compile time, not per pixel).
#[inline]
fn pixel<const BGRA: bool>(row: &[u8], offset: usize) -> (u32, u32, u32) {
    match row.get(offset..offset + 4) {
        Some(&[a, b, c, _]) if BGRA => (u32::from(c), u32::from(b), u32::from(a)),
        Some(&[a, b, c, _]) => (u32::from(a), u32::from(b), u32::from(c)),
        _ => (0, 0, 0),
    }
}

/// Average of the samples under one output pixel (see [`Frame::sample`]).
#[inline]
fn area<const BGRA: bool>(near: &[u8], far: &[u8], (xa, xb): (usize, usize)) -> (u32, u32, u32) {
    let a = pixel::<BGRA>(near, xa);
    let b = pixel::<BGRA>(near, xb);
    let c = pixel::<BGRA>(far, xa);
    let d = pixel::<BGRA>(far, xb);
    (
        (a.0 + b.0 + c.0 + d.0) / 4,
        (a.1 + b.1 + c.1 + d.1) / 4,
        (a.2 + b.2 + c.2 + d.2) / 4,
    )
}

/// The frame scaled to `size` (even sides) and converted to I420. `None` if
/// the frame or the size are not valid.
///
/// Same result as averaging [`Frame::sample`] for each pixel, but the
/// source positions of each column and row are worked out once, rows are
/// read as slices and a picture of the same size is read once per pixel:
/// the costly part of streaming a screen.
pub fn to_i420(frame: &Frame<'_>, size: (usize, usize)) -> Option<Yuv> {
    let (w, h) = size;
    if !frame.valid() || w == 0 || h == 0 || w % 2 == 1 || h % 2 == 1 {
        return None;
    }
    let mut data = vec![0u8; w * h * 3 / 2];
    match frame.order {
        PixelOrder::Bgra => fill_i420::<true>(frame, w, h, &mut data),
        PixelOrder::Rgba => fill_i420::<false>(frame, w, h, &mut data),
    }
    Yuv::new(w, h, data)
}

fn fill_i420<const BGRA: bool>(frame: &Frame<'_>, w: usize, h: usize, data: &mut [u8]) {
    let same_size = (w, h) == (frame.width, frame.height);
    // Byte offsets of the samples of each column.
    let columns: Vec<(usize, usize)> = taps(w, frame.width)
        .into_iter()
        .map(|(a, b)| (a * 4, b * 4))
        .collect();
    let rows = taps(h, frame.height);
    let (y_plane, uv) = data.split_at_mut(w * h);
    let (u_plane, v_plane) = uv.split_at_mut(w * h / 4);
    let lines = y_plane
        .chunks_exact_mut(2 * w)
        .zip(u_plane.chunks_exact_mut(w / 2))
        .zip(v_plane.chunks_exact_mut(w / 2))
        .zip(rows.as_chunks::<2>().0);
    for (((luma_rows, u_row), v_row), taps_y) in lines {
        let (top, bottom) = luma_rows.split_at_mut(w);
        // The source rows under the two output rows (two samples each).
        let (top_near, top_far) = (source_row(frame, taps_y[0].0), source_row(frame, taps_y[0].1));
        let (bottom_near, bottom_far) = (source_row(frame, taps_y[1].0), source_row(frame, taps_y[1].1));
        let blocks = top
            .as_chunks_mut::<2>()
            .0
            .iter_mut()
            .zip(bottom.as_chunks_mut::<2>().0.iter_mut())
            .zip(u_row.iter_mut().zip(v_row.iter_mut()))
            .zip(columns.as_chunks::<2>().0);
        for (((top_pair, bottom_pair), (u, v)), taps_x) in blocks {
            // Four luma samples, their average for the chroma.
            let samples = if same_size {
                // Every sample is the pixel itself: read it once.
                let x = taps_x[0].0;
                [
                    pixel::<BGRA>(top_near, x),
                    pixel::<BGRA>(top_near, x + 4),
                    pixel::<BGRA>(bottom_near, x),
                    pixel::<BGRA>(bottom_near, x + 4),
                ]
            } else {
                [
                    area::<BGRA>(top_near, top_far, taps_x[0]),
                    area::<BGRA>(top_near, top_far, taps_x[1]),
                    area::<BGRA>(bottom_near, bottom_far, taps_x[0]),
                    area::<BGRA>(bottom_near, bottom_far, taps_x[1]),
                ]
            };
            let mut lumas = [0u8; 4];
            let mut sum = (0, 0, 0);
            for (out, (r, g, b)) in lumas.iter_mut().zip(samples) {
                *out = luma(r, g, b);
                sum = (sum.0 + r, sum.1 + g, sum.2 + b);
            }
            *top_pair = [lumas[0], lumas[1]];
            *bottom_pair = [lumas[2], lumas[3]];
            (*u, *v) = chroma(sum.0 / 4, sum.1 / 4, sum.2 / 4);
        }
    }
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

    /// The conversion as it was first written: [`Frame::sample`] for each
    /// pixel. The fast one must give the same bytes.
    fn reference_i420(frame: &Frame<'_>, (w, h): (usize, usize)) -> Vec<u8> {
        let sx = ((frame.width as u64) << 16) / w as u64;
        let sy = ((frame.height as u64) << 16) / h as u64;
        let mut data = vec![0u8; w * h * 3 / 2];
        for oy in (0..h).step_by(2) {
            for ox in (0..w).step_by(2) {
                let mut sum = (0, 0, 0);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (r, g, b) = frame.sample(ox + dx, oy + dy, sx, sy);
                    data[(oy + dy) * w + ox + dx] = luma(r, g, b);
                    sum = (sum.0 + r, sum.1 + g, sum.2 + b);
                }
                let (u, v) = chroma(sum.0 / 4, sum.1 / 4, sum.2 / 4);
                let ci = (oy / 2) * (w / 2) + ox / 2;
                data[w * h + ci] = u;
                data[w * h + w * h / 4 + ci] = v;
            }
        }
        data
    }

    /// A picture with every pixel different (and padding at the end of rows).
    fn pattern(w: usize, h: usize, pitch: usize) -> Vec<u8> {
        let mut data = vec![0xAAu8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * pitch + x * 4;
                let v = (x * 7 + y * 13) as u32;
                data[i..i + 4].copy_from_slice(&[(v % 251) as u8, (v * 3 % 241) as u8, (x ^ y) as u8, 255]);
            }
        }
        data
    }

    #[test]
    fn fast_conversion_matches_the_reference() {
        for (w, h, pitch) in [
            (64, 36, 64 * 4 + 32),
            (101, 57, 101 * 4),
            (40, 30, 40 * 4 + 4),
            (7, 5, 28),
        ] {
            let data = pattern(w, h, pitch);
            for order in [PixelOrder::Bgra, PixelOrder::Rgba] {
                let frame = Frame {
                    data: &data,
                    width: w,
                    height: h,
                    pitch,
                    order,
                };
                // Shrunk, same size (when even), enlarged.
                for size in [(32, 18), (w & !1, h & !1), ((w * 2) & !1, (h * 2) & !1), (16, 16)] {
                    if size.0 == 0 || size.1 == 0 {
                        continue;
                    }
                    let fast = to_i420(&frame, size).unwrap();
                    assert_eq!(
                        fast.data,
                        reference_i420(&frame, size),
                        "{w}x{h} -> {size:?} {order:?}"
                    );
                }
            }
        }
    }

    /// Times the conversion of a full HD screen:
    /// `cargo test --release -- --ignored conversion_speed --nocapture`.
    #[test]
    #[ignore = "a timing, not a check"]
    fn conversion_speed() {
        let (w, h) = (1920, 1080);
        let data = pattern(w, h, w * 4);
        let frame = Frame {
            data: &data,
            width: w,
            height: h,
            pitch: w * 4,
            order: PixelOrder::Bgra,
        };
        for size in [(1280, 720), (1920, 1080)] {
            let runs = 30;
            let start = std::time::Instant::now();
            for _ in 0..runs {
                std::hint::black_box(to_i420(&frame, size));
            }
            let fast = start.elapsed() / runs;
            let start = std::time::Instant::now();
            for _ in 0..runs {
                std::hint::black_box(reference_i420(&frame, size));
            }
            let reference = start.elapsed() / runs;
            println!("1920x1080 -> {size:?}: {fast:?} (before: {reference:?})");
        }
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
