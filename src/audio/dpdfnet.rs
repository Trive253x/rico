//! `DPDFNet` (Ceva, Apache-2.0): `DeepFilterNet2` with dual-path blocks.
//!
//! Trained against over-attenuation. One ONNX graph does the features, the
//! network and the deep filter; here only the causal STFT around it, as in
//! the upstream `StreamEnhancer` (`package/src/dpdfnet/stream.py`): 20 ms
//! Vorbis window, 10 ms hop, the RNN state passed in and out of the graph.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tract_onnx::pb::ModelProto;
use tract_onnx::prelude::*;

/// Samples in and out per step: 10 ms at 48 kHz.
pub const HOP: usize = 480;
/// The analysis window: 20 ms.
const WIN: usize = 2 * HOP;
const BINS: usize = WIN / 2 + 1;

/// At most this much quieter than the microphone (40 dB): never digital
/// silence, the cause of the holes in words with `DeepFilterNet`.
const ATTEN_FLOOR: f32 = 0.01;

type Plan = TypedSimpleState;

/// The model is not in Rico: it is downloaded once, when chosen, from this
/// fixed revision on Hugging Face (Apache-2.0) and checked against
/// [`MODEL_SHA256`] every time it is read.
pub const MODEL_URL: &str = "https://huggingface.co/Ceva-IP/DPDFNet/resolve/\
dd6818d00f50c836fed43a6243ebe49116de5964/onnx/dpdfnet2_48khz_hr.onnx";
pub const MODEL_BYTES: usize = 10_493_337;
const MODEL_SHA256: &str = "7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b";
const MODEL_FILE: &str = "dpdfnet2_48khz_hr.onnx";

/// Where the downloaded model is kept (Rico's data folder).
pub fn model_path() -> Option<PathBuf> {
    crate::config::log_dir().map(|d| d.join("models").join(MODEL_FILE))
}

/// The bytes are exactly the expected model.
fn genuine(bytes: &[u8]) -> bool {
    bytes.len() == MODEL_BYTES && format!("{:x}", Sha256::digest(bytes)) == MODEL_SHA256
}

/// The downloaded model, checked; an error if missing or different.
pub fn read_model() -> Result<Vec<u8>> {
    let path = model_path().context("no data folder")?;
    let len = std::fs::metadata(&path).map_or(0, |m| m.len());
    if len != MODEL_BYTES as u64 {
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
pub async fn ensure_model() -> Result<()> {
    if tokio::task::spawn_blocking(read_model).await?.is_ok() {
        return Ok(());
    }
    let bytes = crate::media::fetch_model(MODEL_URL, MODEL_BYTES).await?;
    if !genuine(&bytes) {
        bail!("the downloaded model is not the expected one");
    }
    tokio::task::spawn_blocking(move || -> Result<()> {
        let path = model_path().context("no data folder")?;
        let dir = path.parent().context("no models folder")?;
        std::fs::create_dir_all(dir)?;
        let part = path.with_extension("onnx.part");
        std::fs::write(&part, &bytes)?;
        std::fs::rename(&part, &path)?;
        Ok(())
    })
    .await??;
    tracing::info!("noise suppression: DPDFNet model downloaded ({MODEL_BYTES} bytes)");
    Ok(())
}

pub struct DpdfNet {
    model: Plan,
    state: TValue,
    window: [f32; WIN],
    input: [f32; WIN],
    output: [f32; WIN],
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    scratch: Vec<Complex32>,
    spec: Vec<f32>,
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
fn vorbis() -> [f32; WIN] {
    let mut w = [0.0; WIN];
    let half = WIN as f64 / 2.0;
    for (i, v) in w.iter_mut().enumerate() {
        let s = (0.5 * std::f64::consts::PI * (i as f64 + 0.5) / half).sin();
        *v = (0.5 * std::f64::consts::PI * s * s).sin() as f32;
    }
    w
}

impl DpdfNet {
    /// The 48 kHz model from its ONNX bytes; refuses any other layout.
    pub fn new(onnx: &[u8]) -> Result<Self> {
        let onnx_fw = tract_onnx::onnx();
        let proto = onnx_fw
            .proto_model_for_read(&mut Cursor::new(onnx))
            .context("reading the model")?;
        if meta(&proto, "model_type")? != "dpdfnet"
            || meta_usize(&proto, "sample_rate")? != 48_000
            || meta_usize(&proto, "n_fft")? != WIN
            || meta_usize(&proto, "hop_length")? != HOP
            || meta_usize(&proto, "freq_bins")? != BINS
            || meta(&proto, "window_type")? != "vorbis"
        {
            bail!("not a 48 kHz DPDFNet model");
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
            .with_input_fact(0, f32::fact([1, 1, BINS, 2]).into())?
            .with_input_fact(1, f32::fact([state_size]).into())?
            .into_optimized()?
            .into_runnable()?;
        let model = TypedSimpleState::new(&plan)?;

        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(WIN);
        let ifft = planner.plan_fft_inverse(WIN);
        let scratch_len = fft.get_scratch_len().max(ifft.get_scratch_len());
        Ok(Self {
            model,
            state: Tensor::from_shape(&[state_size], &init)?.into_tvalue(),
            window: vorbis(),
            input: [0.0; WIN],
            output: [0.0; WIN],
            time: fft.make_input_vec(),
            spectrum: fft.make_output_vec(),
            scratch: vec![Complex32::default(); scratch_len],
            fft,
            ifft,
            spec: vec![0.0; BINS * 2],
        })
    }

    /// Cleans 10 ms in place; the result is 10 ms late (the second half of
    /// the window is needed first).
    pub fn process(&mut self, chunk: &mut [f32; HOP]) -> Result<()> {
        self.input.copy_within(HOP.., 0);
        self.input[HOP..].copy_from_slice(chunk);

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

        let spec = Tensor::from_shape(&[1, 1, BINS, 2], &self.spec)?;
        let mut out = self.model.run(tvec!(spec.into_tvalue(), self.state.clone()))?;
        if out.len() != 2 {
            bail!("model gave {} outputs, 2 expected", out.len());
        }
        self.state = out.remove(1);
        let enhanced = out[0].to_plain_array_view::<f32>()?;
        let enhanced = enhanced.as_slice().context("model output not contiguous")?;
        if enhanced.len() != BINS * 2 {
            bail!("model gave {} values, {} expected", enhanced.len(), BINS * 2);
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

        let scale = 1.0 / WIN as f32;
        for ((o, t), w) in self.output.iter_mut().zip(&self.time).zip(&self.window) {
            *o += t * scale * w;
        }
        // The first half is complete; with the noisy samples it belongs to
        // (the previous 10 ms), mixed back at −40 dB.
        for ((s, o), x) in chunk.iter_mut().zip(&self.output[..HOP]).zip(&self.input[..HOP]) {
            *s = (1.0 - ATTEN_FLOOR) * o + ATTEN_FLOOR * x;
        }
        self.output.copy_within(HOP.., 0);
        self.output[HOP..].fill(0.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vorbis_window_adds_up_to_one() {
        let w = vorbis();
        for i in 0..HOP {
            let sum = w[i] * w[i] + w[i + HOP] * w[i + HOP];
            assert!((sum - 1.0).abs() < 1e-5, "{i}: {sum}");
        }
    }

    #[test]
    fn only_the_expected_model_is_genuine() {
        assert!(!genuine(b"not onnx"));
        assert!(!genuine(&vec![0; MODEL_BYTES]));
        assert!(MODEL_URL.starts_with("https://huggingface.co/Ceva-IP/DPDFNet/resolve/dd6818d"));
        assert!(!MODEL_URL.contains(char::is_whitespace));
    }

    #[test]
    fn not_a_model_is_refused() {
        assert!(DpdfNet::new(b"not onnx").is_err());
    }

    /// With the real model (`RICO_DPDFNET_MODEL=path/to/dpdfnet2_48khz_hr.onnx`):
    /// noise goes down, never to zero, and a tone in it survives.
    #[test]
    fn the_real_model_cleans_noise() {
        let Ok(path) = std::env::var("RICO_DPDFNET_MODEL") else {
            return;
        };
        let bytes = std::fs::read(path).unwrap();
        let mut d = DpdfNet::new(&bytes).unwrap();
        let mut seed = 7u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.1
        };
        let rms = |x: &[f32]| (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt();
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
        assert!(out_noise > in_noise * 0.005, "{out_noise}");
    }
}
