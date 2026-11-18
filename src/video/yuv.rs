//! Decoded pictures (I420) to RGBA, for showing a stream.
//!
//! The `OpenH264` crate's own conversion uses BT.601 colours and repeats each
//! colour sample over 2×2 pixels: on HD streams (BT.709, as browsers and
//! Discord's client read them) the colours came out shifted and coloured
//! edges blocky. Here: BT.709 for HD pictures (720 lines and more) and BT.601
//! below, the usual choice when the stream does not say; limited range; the
//! colour planes interpolated (horizontally co-sited, vertically centred, as
//! H.264 4:2:0 places them). Integer maths, one pass per row.

/// Colour matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
}

impl Matrix {
    /// HD pictures are BT.709, smaller ones BT.601.
    pub fn for_height(height: usize) -> Self {
        if height >= 720 {
            Self::Bt709
        } else {
            Self::Bt601
        }
    }

    /// (R from V, G from U, G from V, B from U), scaled by 2^16, for
    /// limited-range input (chroma already × 4 from the interpolation).
    fn coefficients(self) -> (i32, i32, i32, i32) {
        // 255/224 × the matrix terms, × 65536.
        match self {
            Self::Bt601 => (104_597, -25_675, -53_279, 132_201),
            Self::Bt709 => (117_489, -13_975, -34_925, 138_438),
        }
    }
}

/// 255/219 × 65536: limited-range luma to full range.
const Y_SCALE: i32 = 76_309;

/// Planes of an I420 picture.
pub struct Planes<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    /// Row lengths in bytes of the Y, U and V planes.
    pub strides: (usize, usize, usize),
}

/// Converts a `width`×`height` I420 picture to RGBA. `None` if the planes are
/// too short for the size (never reads out of them).
pub fn to_rgba(p: &Planes<'_>, width: usize, height: usize, matrix: Matrix) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
    let (ys, us, vs) = p.strides;
    if ys < width || us < cw || vs < cw {
        return None;
    }
    if p.y.len() < ys * (height - 1) + width
        || p.u.len() < us * (ch - 1) + cw
        || p.v.len() < vs * (ch - 1) + cw
    {
        return None;
    }
    let (rv, gu, gv, bu) = matrix.coefficients();
    let mut out = vec![0u8; width * height * 4];
    // One row of chroma, interpolated vertically, ×4.
    let mut u_row = vec![0i32; cw];
    let mut v_row = vec![0i32; cw];
    for (row, line) in out.chunks_exact_mut(width * 4).enumerate() {
        // Vertically centred: an even luma row is 1/4 of the chroma row above
        // and 3/4 of its own, an odd one 3/4 of its own and 1/4 of the next.
        let c = row / 2;
        let other = if row % 2 == 0 {
            c.saturating_sub(1)
        } else {
            (c + 1).min(ch - 1)
        };
        let (u_a, u_b) = (&p.u[c * us..c * us + cw], &p.u[other * us..other * us + cw]);
        let (v_a, v_b) = (&p.v[c * vs..c * vs + cw], &p.v[other * vs..other * vs + cw]);
        for i in 0..cw {
            u_row[i] = 3 * i32::from(u_a[i]) + i32::from(u_b[i]) - 4 * 128;
            v_row[i] = 3 * i32::from(v_a[i]) + i32::from(v_b[i]) - 4 * 128;
        }
        let y_line = &p.y[row * ys..row * ys + width];
        for (x, (px, &luma)) in line.chunks_exact_mut(4).zip(y_line).enumerate() {
            // Horizontally co-sited: even columns on a sample, odd ones half way.
            let i = x / 2;
            let (u, v) = if x % 2 == 0 {
                (u_row[i] * 2, v_row[i] * 2)
            } else {
                let j = (i + 1).min(cw - 1);
                (u_row[i] + u_row[j], v_row[i] + v_row[j])
            };
            // u, v are ×8 here: the coefficients are ×65536, so shift by 19.
            let yy = (i32::from(luma) - 16) * Y_SCALE;
            let r = (yy + ((rv * v) >> 3)) >> 16;
            let g = (yy + ((gu * u + gv * v) >> 3)) >> 16;
            let b = (yy + ((bu * u) >> 3)) >> 16;
            px[0] = clamp(r);
            px[1] = clamp(g);
            px[2] = clamp(b);
            px[3] = 255;
        }
    }
    Some(out)
}

#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn clamp(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, y: u8, u: u8, v: u8, matrix: Matrix) -> Vec<u8> {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let (yp, up, vp) = (vec![y; w * h], vec![u; cw * ch], vec![v; cw * ch]);
        to_rgba(
            &Planes {
                y: &yp,
                u: &up,
                v: &vp,
                strides: (w, cw, cw),
            },
            w,
            h,
            matrix,
        )
        .unwrap()
    }

    fn near(a: &[u8], b: [u8; 3]) -> bool {
        a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 2)
    }

    #[test]
    fn known_colours() {
        for m in [Matrix::Bt601, Matrix::Bt709] {
            // Black, white and grey: the same in both matrices.
            assert!(near(&flat(4, 4, 16, 128, 128, m)[..3], [0, 0, 0]));
            assert!(near(&flat(4, 4, 235, 128, 128, m)[..3], [255, 255, 255]));
            assert!(near(&flat(4, 4, 126, 128, 128, m)[..3], [128, 128, 128]));
        }
        // Pure red in each matrix (limited range).
        assert!(near(&flat(4, 4, 81, 90, 240, Matrix::Bt601)[..3], [255, 0, 0]));
        assert!(near(&flat(4, 4, 63, 102, 240, Matrix::Bt709)[..3], [255, 0, 0]));
        assert_eq!(flat(4, 4, 81, 90, 240, Matrix::Bt601)[3], 255);
    }

    #[test]
    fn colour_edges_are_smoothed() {
        // 4×2 with chroma red on the left pair, neutral on the right.
        let (w, h) = (4, 2);
        let y = vec![81u8; w * h];
        let (u, v) = (vec![90u8, 128], vec![240u8, 128]);
        let out = to_rgba(
            &Planes {
                y: &y,
                u: &u,
                v: &v,
                strides: (w, 2, 2),
            },
            w,
            h,
            Matrix::Bt601,
        )
        .unwrap();
        // Column 1 is half way between the two samples: less red than column 0.
        assert!(out[4] < out[0] && out[4] > out[8]);
    }

    #[test]
    fn short_planes_are_refused() {
        let y = vec![0u8; 10];
        let c = vec![0u8; 4];
        let p = Planes {
            y: &y,
            u: &c,
            v: &c,
            strides: (4, 2, 2),
        };
        assert!(to_rgba(&p, 4, 4, Matrix::Bt601).is_none());
        assert!(to_rgba(&p, 0, 4, Matrix::Bt601).is_none());
        assert_eq!(Matrix::for_height(720), Matrix::Bt709);
        assert_eq!(Matrix::for_height(480), Matrix::Bt601);
    }
}
