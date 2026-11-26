//! `DPDFNet` (Ceva, Apache-2.0): `DeepFilterNet2` with dual-path blocks.
//!
//! Trained against over-attenuation. One ONNX graph does the features, the
//! network and the deep filter; here only the causal STFT around it, as in
//! the upstream `StreamEnhancer` (`package/src/dpdfnet/stream.py`): 20 ms
//! Vorbis window, 10 ms hop, the RNN state passed in and out of the graph.
//!
//! The model is the 16 kHz `baseline` (0.36 GMACs a second). Rico runs at
//! 48 kHz: the microphone is filtered down to 16 kHz before the model and
//! back up after it, so the cleaned voice keeps no sound above 8 kHz. The
//! 48 kHz model (`dpdfnet2_48khz_hr`) was removed in 0.20.0: more than twice
//! the CPU, and too slow on a busy computer.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tract_onnx::pb::ModelProto;
use tract_onnx::prelude::*;

/// Samples in and out per step: 10 ms at 48 kHz (Rico's rate).
pub const HOP: usize = 480;

/// The fixed revision on Hugging Face the models come from.
const REVISION: &str = "dd6818d00f50c836fed43a6243ebe49116de5964";

/// The model's file name, on Hugging Face and on disk.
const FILE: &str = "baseline.onnx";
/// Exact size of the file.
pub const BYTES: usize = 8_722_707;
const SHA256: &str = "371d26182aff0e1e0d31354e24c81f79cd57458f5a7dd003fc10a7ebf64255e0";
/// The model's own sample rate.
const RATE: usize = 16_000;
/// The 48 kHz model's file, removed in 0.20.0: deleted from disk if found.
const RETIRED: &str = "dpdfnet2_48khz_hr.onnx";

/// Where the model is downloaded from (only Hugging Face, a fixed revision).
pub fn url() -> String {
    format!("https://huggingface.co/Ceva-IP/DPDFNet/resolve/{REVISION}/onnx/{FILE}")
}

/// Where the downloaded model is kept (Rico's data folder).
pub fn path() -> Option<PathBuf> {
    crate::config::log_dir().map(|d| d.join("models").join(FILE))
}

/// The bytes are exactly the expected model.
fn genuine(bytes: &[u8]) -> bool {
    bytes.len() == BYTES && format!("{:x}", Sha256::digest(bytes)) == SHA256
}

/// The model is on disk with the right size (cheap: not read or checked;
/// [`read`] checks it before use).
pub fn downloaded() -> bool {
    path()
        .and_then(|p| std::fs::metadata(p).ok())
        .is_some_and(|m| m.len() == BYTES as u64)
}

/// Deletes the removed 48 kHz model, if it was downloaded (10.5 MB that
/// nothing uses any more). Best effort.
pub fn remove_retired() {
    let Some(old) = crate::config::log_dir().map(|d| d.join("models").join(RETIRED)) else {
        return;
    };
    match std::fs::remove_file(&old) {
        Ok(()) => tracing::info!("noise suppression: the removed 48 kHz DPDFNet model was deleted"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("noise suppression: could not delete the removed model: {e}"),
    }
}

/// The downloaded model, checked; an error if missing or different.
pub fn read() -> Result<Vec<u8>> {
    let path = path().context("no data folder")?;
    let len = std::fs::metadata(&path).map_or(0, |m| m.len());
    if len != BYTES as u64 {
        bail!("the DPDFNet model is not downloaded");
    }
    let bytes = std::fs::read(&path).context("reading the DPDFNet model")?;
    if !genuine(&bytes) {
        bail!("the DPDFNet model on disk is not the expected one");
    }
    Ok(bytes)
}

/// Makes sure the model is on disk: kept if already there and genuine,
/// otherwise downloaded (only from Hugging Face), checked, then saved.
pub async fn ensure() -> Result<()> {
    if tokio::task::spawn_blocking(read).await?.is_ok() {
        return Ok(());
    }
    let bytes = crate::media::fetch_model(&url(), BYTES).await?;
    if !genuine(&bytes) {
        bail!("the downloaded model is not the expected one");
    }
    tokio::task::spawn_blocking(move || -> Result<()> {
        let path = path().context("no data folder")?;
        let dir = path.parent().context("no models folder")?;
        std::fs::create_dir_all(dir)?;
        // A name of its own: two downloads never share the file.
        let part = path.with_extension(format!("onnx.{}.part", std::process::id()));
        std::fs::write(&part, &bytes)?;
        std::fs::rename(&part, &path)?;
        Ok(())
    })
    .await??;
    tracing::info!("noise suppression: DPDFNet model downloaded ({BYTES} bytes)");
    Ok(())
}

/// One `key=value` of the model's metadata.
fn meta<'a>(proto: &'a ModelProto, key: &str) -> Result<&'a str> {
    proto
        .metadata_props
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.value.as_str())
        .with_context(|| format!("model metadata without {key}"))
}

fn meta_usize(proto: &ModelProto, key: &str) -> Result<usize> {
    meta(proto, key)?
        .trim()
        .parse()
        .with_context(|| format!("model metadata {key} is not a number"))
}

fn meta_floats(proto: &ModelProto, key: &str, len: usize) -> Result<Vec<f32>> {
    let values = meta(proto, key)?
        .split(',')
        .map(|v| v.trim().parse::<f32>())
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("model metadata {key} is not a list of numbers"))?;
    if values.len() != len {
        bail!("model metadata {key}: {} values, {len} expected", values.len());
    }
    Ok(values)
}

/// The Vorbis window (power complementary at 50% overlap: w²[n] + w²[n+hop] = 1).
fn vorbis(len: usize) -> Vec<f32> {
    let half = len as f64 / 2.0;
    (0..len)
        .map(|i| {
            let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / half).sin();
            (0.5 * std::f64::consts::PI * s * s).sin() as f32
        })
        .collect()
}

/// Taps of the low-pass filter between 48 and 16 kHz: windowed sinc
/// (Blackman), cut at 7.2 kHz, unit gain. 2.0 ms long, 1 ms of delay each way.
const FIR_TAPS: usize = 97;

fn lowpass() -> Vec<f32> {
    let cutoff = 7_200.0 / 48_000.0;
    let mid = (FIR_TAPS - 1) as f64 / 2.0;
    let mut taps: Vec<f64> = (0..FIR_TAPS)
        .map(|i| {
            let x = i as f64 - mid;
            let sinc = if x == 0.0 {
                2.0 * cutoff
            } else {
                (2.0 * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x)
            };
            let n = i as f64 / (FIR_TAPS - 1) as f64;
            let blackman = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * n).cos()
                + 0.08 * (4.0 * std::f64::consts::PI * n).cos();
            sinc * blackman
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter_mut().for_each(|t| *t /= sum);
    taps.into_iter().map(|t| t as f32).collect()
}

/// A streaming FIR filter at 48 kHz: keeps the last taps−1 input samples.
struct Fir {
    taps: Vec<f32>,
    /// History, then the new block.
    buf: Vec<f32>,
}

impl Fir {
    fn new() -> Self {
        Self {
            taps: lowpass(),
            buf: vec![0.0; FIR_TAPS - 1 + HOP],
        }
    }

    /// Filters `input` (48 kHz, [`HOP`] samples); `out` gets every `step`-th
    /// output sample, multiplied by `gain`.
    fn run(&mut self, input: &[f32], step: usize, gain: f32, out: &mut [f32]) {
        let hist = FIR_TAPS - 1;
        self.buf[hist..].copy_from_slice(input);
        for (k, o) in out.iter_mut().enumerate() {
            let end = hist + k * step;
            let window = &self.buf[end + 1 - FIR_TAPS..=end];
            *o = gain
                * window
                    .iter()
                    .rev()
                    .zip(&self.taps)
                    .map(|(x, t)| x * t)
                    .sum::<f32>();
        }
        self.buf.copy_within(HOP.., 0);
    }
}

/// 48 kHz ↔ 16 kHz around the light model.
struct Resampler {
    down: Fir,
    up: Fir,
    stuffed: Vec<f32>,
}

impl Resampler {
    fn new() -> Self {
        Self {
            down: Fir::new(),
            up: Fir::new(),
            stuffed: vec![0.0; HOP],
        }
    }

    fn down(&mut self, input: &[f32], out: &mut [f32]) {
        self.down.run(input, 3, 1.0, out);
    }

    fn up(&mut self, input: &[f32], out: &mut [f32]) {
        self.stuffed.fill(0.0);
        for (i, x) in input.iter().enumerate() {
            self.stuffed[i * 3] = *x;
        }
        let stuffed = std::mem::take(&mut self.stuffed);
        self.up.run(&stuffed, 1, 3.0, out);
        self.stuffed = stuffed;
    }
}

pub struct DpdfNet {
    model: TypedSimpleState,
    state: TValue,
    /// The model's own step and window (samples at its rate).
    hop: usize,
    bins: usize,
    window: Vec<f32>,
    input: Vec<f32>,
    output: Vec<f32>,
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    spec: Vec<f32>,
    /// 48 kHz ↔ the model's 16 kHz.
    resampler: Resampler,
    low: Vec<f32>,
}

impl DpdfNet {
    /// A model from its ONNX bytes; refuses any other layout.
    pub fn new(onnx: &[u8]) -> Result<Self> {
        let rate = RATE;
        let hop = rate / 100;
        let win = 2 * hop;
        let bins = win / 2 + 1;
        let onnx_fw = tract_onnx::onnx();
        let proto = onnx_fw
            .proto_model_for_read(&mut Cursor::new(onnx))
            .context("reading the model")?;
        if meta(&proto, "model_type")? != "dpdfnet"
            || meta_usize(&proto, "sample_rate")? != rate
            || meta_usize(&proto, "n_fft")? != win
            || meta_usize(&proto, "hop_length")? != hop
            || meta_usize(&proto, "freq_bins")? != bins
            || meta(&proto, "window_type")? != "vorbis"
        {
            bail!("not a {} kHz DPDFNet model", rate / 1000);
        }
        let state_size = meta_usize(&proto, "state_size")?;
        let erb = meta_usize(&proto, "erb_norm_state_size")?;
        let spec = meta_usize(&proto, "spec_norm_state_size")?;
        if erb + spec > state_size {
            bail!("model state sizes do not add up");
        }
        let mut init = vec![0f32; state_size];
        init[..erb].copy_from_slice(&meta_floats(&proto, "erb_norm_init", erb)?);
        init[erb..erb + spec].copy_from_slice(&meta_floats(&proto, "spec_norm_init", spec)?);

        let plan = onnx_fw
            .model_for_proto_model(&proto)
            .context("translating the model")?
            .with_input_fact(0, f32::fact([1, 1, bins, 2]).into())?
            .with_input_fact(1, f32::fact([state_size]).into())?
            .into_optimized()?
            .into_runnable()?;
        let model = TypedSimpleState::new(&plan)?;

        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(win);
        let ifft = planner.plan_fft_inverse(win);
        let scratch_len = fft.get_scratch_len().max(ifft.get_scratch_len());
        Ok(Self {
            model,
            state: Tensor::from_shape(&[state_size], &init)?.into_tvalue(),
            hop,
            bins,
            window: vorbis(win),
            input: vec![0.0; win],
            output: vec![0.0; win],
            time: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: vec![Complex32::default(); scratch_len],
            fft,
            ifft,
            spec: vec![0.0; bins * 2],
            resampler: Resampler::new(),
            low: vec![0.0; hop],
        })
    }

    /// Cleans 10 ms at 48 kHz in place; the result is 10 ms late (the
    /// second half of the window is needed first), plus 2 ms for the
    /// filters to and from 16 kHz.
    pub fn process(&mut self, chunk: &mut [f32; HOP]) -> Result<()> {
        let mut low = std::mem::take(&mut self.low);
        self.resampler.down(chunk, &mut low);
        let result = self.step(&mut low);
        self.resampler.up(&low, chunk);
        self.low = low;
        result
    }

    /// One step of the model at its own rate: `hop` samples in place.
    fn step(&mut self, samples: &mut [f32]) -> Result<()> {
        let hop = self.hop;
        if samples.len() != hop {
            bail!("{} samples, {hop} expected", samples.len());
        }
        self.input.copy_within(hop.., 0);
        self.input[hop..].copy_from_slice(samples);

        for ((t, x), w) in self.time.iter_mut().zip(&self.input).zip(&self.window) {
            *t = x * w;
        }
        self.fft
            .process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for (pair, c) in self.spec.as_chunks_mut::<2>().0.iter_mut().zip(&self.spectrum) {
            pair[0] = c.re;
            pair[1] = c.im;
        }

        let spec = Tensor::from_shape(&[1, 1, self.bins, 2], &self.spec)?;
        let mut out = self.model.run(tvec!(spec.into_tvalue(), self.state.clone()))?;
        if out.len() != 2 {
            bail!("model gave {} outputs, 2 expected", out.len());
        }
        self.state = out.remove(1);
        let enhanced = out[0].to_plain_array_view::<f32>()?;
        let enhanced = enhanced.as_slice().context("model output not contiguous")?;
        if enhanced.len() != self.bins * 2 {
            bail!("model gave {} values, {} expected", enhanced.len(), self.bins * 2);
        }
        for (c, pair) in self.spectrum.iter_mut().zip(enhanced.as_chunks::<2>().0) {
            *c = Complex32::new(pair[0], pair[1]);
        }
        // A real signal: no imaginary part at 0 Hz and at Nyquist.
        if let Some(c) = self.spectrum.first_mut() {
            c.im = 0.0;
        }
        if let Some(c) = self.spectrum.last_mut() {
            c.im = 0.0;
        }
        self.ifft
            .process_with_scratch(&mut self.spectrum, &mut self.time, &mut self.scratch)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let scale = 1.0 / self.window.len() as f32;
        for ((o, t), w) in self.output.iter_mut().zip(&self.time).zip(&self.window) {
            *o += t * scale * w;
        }
        // The first half is complete. The model's own output, as upstream
        // gives it (no attenuation limit: the owner's choice).
        samples.copy_from_slice(&self.output[..hop]);
        self.output.copy_within(hop.., 0);
        self.output[hop..].fill(0.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vorbis_window_adds_up_to_one() {
        for len in [320, 960] {
            let w = vorbis(len);
            let hop = len / 2;
            for i in 0..hop {
                let sum = w[i] * w[i] + w[i + hop] * w[i + hop];
                assert!((sum - 1.0).abs() < 1e-5, "{len} {i}: {sum}");
            }
        }
    }

    #[test]
    fn only_the_expected_model_is_genuine() {
        assert!(!genuine(b"not onnx"));
        assert!(!genuine(&vec![0; BYTES]));
        assert!(url().starts_with("https://huggingface.co/Ceva-IP/DPDFNet/resolve/dd6818d"));
        assert!(!url().contains(char::is_whitespace));
        assert!(url().ends_with("/onnx/baseline.onnx"));
    }

    #[test]
    fn not_a_model_is_refused() {
        assert!(DpdfNet::new(b"not onnx").is_err());
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn the_resampler_keeps_the_voice_band_and_cuts_above_8_khz() {
        let tone = |hz: f32, n: usize| {
            (0..n)
                .map(|i| (2.0 * std::f32::consts::PI * hz * i as f32 / 48_000.0).sin() * 0.5)
                .collect::<Vec<_>>()
        };
        for (hz, kept) in [(300.0, true), (3_000.0, true), (6_000.0, true), (12_000.0, false)] {
            let mut r = Resampler::new();
            let input = tone(hz, HOP * 20);
            let mut last = [0f32; HOP];
            for block in input.as_chunks::<HOP>().0 {
                let mut low = [0f32; HOP / 3];
                r.down(block, &mut low);
                r.up(&low, &mut last);
            }
            let ratio = rms(&last) / rms(&input);
            if kept {
                assert!((0.85..1.15).contains(&ratio), "{hz} Hz: {ratio}");
            } else {
                assert!(ratio < 0.05, "{hz} Hz: {ratio}");
            }
        }
    }

    /// With the real model (`RICO_DPDFNET_LIGHT` = its path): the output is
    /// finite and the noise goes down.
    #[test]
    fn the_real_model_cleans_noise() {
        {
            let Ok(path) = std::env::var("RICO_DPDFNET_LIGHT") else {
                return;
            };
            let bytes = std::fs::read(path).unwrap();
            let mut d = DpdfNet::new(&bytes).unwrap();
            let mut seed = 7u32;
            let mut noise = || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.1
            };
            let start = std::time::Instant::now();
            let mut out_noise = 0.0;
            for _ in 0..300 {
                let mut chunk = [0f32; HOP];
                chunk.iter_mut().for_each(|s| *s = noise());
                d.process(&mut chunk).unwrap();
                assert!(chunk.iter().all(|s| s.is_finite()));
                out_noise = rms(&chunk);
            }
            let per_hop = start.elapsed().as_secs_f64() * 1000.0 / 300.0;
            let in_noise = 0.1 / 12f32.sqrt();
            eprintln!("noise {in_noise} -> {out_noise}, {per_hop:.2} ms per 10 ms");
            assert!(out_noise < in_noise * 0.3, "{out_noise}");
        }
    }
}
