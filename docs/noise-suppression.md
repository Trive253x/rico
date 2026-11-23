# Better noise suppression: proposal

> **Status (0.19.0):** DeepFilterNet was removed before the 0.19.0 release
> (exact zeros where it hears no voice: holes in words on consoles). The
> choices are Off, Standard, RNNoise and DPDFNet (section at the end); a
> saved DeepFilterNet choice is read as RNNoise. The DeepFilterNet sections
> below are kept as the record of the evaluation.


Status: **implemented in 0.19.0** (2026-11-21): the owner chose both,
with the choice in the settings (Off, Standard, RNNoise, DeepFilterNet).
`deep-filter-rt` is pinned to `=0.2.0`; review its diff at every update.

## Why

The owner tried the noise suppression of 0.19.0 on Discord and found it
weak. It is WebRTC's (`sonora`, `NoiseSuppressionLevel::High` in
`src/audio/process.rs`): a classical filter that removes steady noise (fan,
hiss, air conditioning) but lets most sudden noise through (keyboard,
clicks, dogs, dishes, voices in the background). Krisp, which Discord uses,
is a neural network that separates the voice from everything else.

**Krisp itself is not possible**: it is a closed SDK, available only under a
commercial contract with Krisp (Discord pays for it).

## The proposal

Replace the "Noise suppression" switch in Settings › Voice & video › Voice
processing with a choice:

| Noise suppression | For |
|---|---|
| **Off** | good microphones, quiet rooms |
| **Standard** (today's, WebRTC) | fan and hiss, almost no CPU |
| **RNNoise** | also keyboard and sudden noise, light |
| **DeepFilterNet** | the strongest, close to Krisp; more CPU, about 30 ms more delay |

- Default stays **Standard**: nothing changes for who updates until they
  choose something else. The old `noise_suppression: bool` setting maps to
  Off / Standard (tolerant reading of the settings file; a missing or
  unknown value means Standard, never a weaker setting by surprise).
- It can be changed during a call and applies at once, to compare them.
- Echo cancellation and automatic gain stay separate switches and work with
  all three.
- If DeepFilterNet cannot start (model fails to load, not enough memory),
  Rico falls back to Standard and says so: never silently without a filter.
- Each choice has a line saying what it removes and what it costs.
- DeepFilterNet runs with an attenuation limit of 30 dB and a "noise only"
  threshold of −15 dB local SNR (its defaults: no limit, −10 dB). With the
  defaults it writes exact zeros where it hears no voice, and soft parts of
  words fell into those holes ("casa" heard as "c-asa", on PlayStation 5
  above all; RNNoise did not do it).
- Like today: runs only while the microphone is on, on the voice
  connection's task (never the interface thread), 10 ms at a time at 48 kHz.
  The DeepFilterNet model is loaded off the interface thread, only when it
  is chosen, and dropped when the call ends or another choice is made.
- Everything runs on the computer: no audio leaves it, no new server.

## Dependency checks (AGENTS.md: new dependencies)

Done on 2026-11-20 in a separate worktree (nothing committed), compared with
`main` at `a85660a`.

### RNNoise: `nnnoiseless` 0.5.2 — passes

- Mature: since 2020, 1.2 million downloads, last release 2025-12-18,
  repository active (github.com/jneem/nnnoiseless), owner `jneem`.
- License BSD-3-Clause, already allowed by `deny.toml`.
- With `default-features = false` it adds only `easyfft` (and the FFT crates
  it uses). No build script, no native code; the C API is behind a feature
  that stays off. One `unsafe` (reinterpreting `&[u8]` weights as `&[i8]`),
  sound.
- About +1.2 MB to the program, about 0.6 % of one core, no extra delay.
- Expects samples in the `i16` range as `f32` (scale by 32767 in and out).

### DeepFilterNet: the official crate cannot be used

- `deep_filter` on crates.io is at 0.2.5 (2022-07-28) and has **no model
  inference** (no `tract`): only the signal processing part.
- Upstream (github.com/Rikorose/DeepFilterNet, `libDF`) has the inference
  but is only usable as a git dependency, which `deny.toml` refuses
  (`unknown-git = "deny"`). An exception would need the owner's approval;
  not recommended.

### DeepFilterNet: `deep-filter-rt` 0.2.0 — passes, with two decisions

- What it is: a vendored copy of upstream `libDF` at commit `d375b2d`
  (2024-10-17), reduced to real-time inference, updated to tract 0.23 and
  ndarray 0.17, with the DeepFilterNet3 model embedded (`include_bytes!`).
  License MIT OR Apache-2.0. Owner `chpio` (gitlab.com/chpio/voice-filter).
- **Model verified**: `config.ini`, `enc.onnx`, `erb_dec.onnx` and
  `df_dec.onnx` are byte for byte the content of upstream's
  `models/DeepFilterNet3_onnx.tar.gz` at `d375b2d` (same SHA-256).
- **Weights license verified**: upstream's README (commit `7b14b89`,
  2026-11-06) says all code and model weights are MIT or Apache-2.0.
- **Code**: about 1100 changed lines against upstream's `tract.rs` and
  `lib.rs` (removals and updates). The `unsafe` blocks come from upstream
  except `plain_mut`, which is sound and commented. No network, processes
  or file writes (one `fs` read for loading a model from a directory, not
  used with the embedded model).
- **Risk**: 19 days old, 3 versions, one maintainer. If chosen: pin
  `=0.2.0` and review the diff at every update.
- **Weight**: 55 new crates, almost all of `tract` (Sonos, well maintained).
  Native code: `tract-linalg` compiles optimized kernels with `cc`. Build
  scripts: `tract-linalg`, `matrixmultiply`, `erased-serde`, `filetime`,
  `tiny-keccak`, `typeid`; proc macros: `const-random-macro`, `derive-new`,
  `prost-derive`. Clearly longer builds.
- **Size**: a test program with both is 37 MB stripped (an empty one
  0.3 MB, RNNoise alone 1.5 MB), 17 MB gzip-compressed. Expect the
  installer to grow by about 12–17 MB (today about 13 MB).
- **Speed** (release, 4-core Xeon 2.1 GHz VM, synthetic signal): about
  3.6–4 % of one core, 0.36 ms per 10 ms frame on average, worst
  1.5–5 ms after the first frames (always inside the 10 ms budget); model
  load 85 ms; 30 ms more delay (10 ms STFT overlap plus 2 lookahead frames).
  Peak memory of the test program about 58 MB.

### Both together

`cargo deny check` and `cargo audit` give exactly the same findings with
the two crates as on `main` today (all already judged by the CI's security
report): no new vulnerability, unmaintained crate, license or source
problem.

Not measured: the quality on a real voice (only synthetic signals here) and
anything on Windows.

## Decisions for the owner

1. RNNoise: add it? (Recommended: yes.)
2. DeepFilterNet through `deep-filter-rt`: accept a dependency that is a few
   days old with one maintainer, pinned to `=0.2.0`?
3. Accept an installer about twice as big (about 13 → 25–30 MB) and longer
   builds for it?

If 2 or 3 is no: RNNoise only, and DeepFilterNet stays in `docs/todo.md`
until a more established crate exists.

## Work, once decided

- `src/audio/mod.rs`: `noise_suppression: bool` → an enum (`Off`,
  `Standard`, `RnNoise`, `DeepFilter`), with the old value read tolerantly.
- `src/audio/process.rs`: the chosen suppressor after the echo canceller
  (WebRTC's own NS only for Standard); RNNoise and DeepFilterNet in 480-sample
  frames at 48 kHz; the fallback to Standard with a message.
- Settings › Voice & video: the choice with its description (English and
  Italian).
- Tests: the settings migration, each choice processing frames, the
  fallback, digital silence, no allocation per frame where possible.
- `Cargo.toml` and `Cargo.lock`, CHANGELOG, README (table and usage),
  `docs/privacy-and-security.md` if needed (no new server), this file's
  status.
- A candidate for the owner to compare the four on Discord.

## DPDFNet (0.19.0, experimental)

A fifth choice, added after DeepFilterNet left holes in words on
consoles. DPDFNet (Ceva, arXiv 2512.16420) is DeepFilterNet2 with dual-path
blocks in the encoder, trained with a loss against over-attenuation; on the
authors' own multilingual low-SNR set it scores above DeepFilterNet2 and 3 on
almost every metric (no independent comparison yet).

- Model: `dpdfnet2_48khz_hr` (48 kHz, 2.58 M parameters, 2.42 GMACs/s),
  ONNX, Apache-2.0 (code on GitHub `ceva-ip/DPDFNet`, weights on Hugging
  Face `Ceva-IP/DPDFNet`, revision `dd6818d00f50c836fed43a6243ebe49116de5964`,
  SHA-256 `7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b`,
  10,493,337 bytes).
- Not in the installer: downloaded once when chosen (owner's choice: the
  repository history stays small), through `media::fetch_model` (HTTPS,
  `huggingface.co` and `*.hf.co` only, redirects followed by hand, size
  limit), checked against the SHA-256 before saving and at every load. In
  the demo nothing is downloaded.
- Interface of the graph: `spec [1,1,481,2]` and `state_in [56436]` in,
  `spec_e` and `state_out` out; features, normalisation and the deep filter
  are inside. Rico does the causal STFT around it as upstream's
  `StreamEnhancer`: Vorbis window of 960 samples, hop 480, overlap-add; the
  initial state comes from the model's metadata. 10 ms of delay from the STFT.
- The model's own output, unchanged, as upstream (owner's choice: no
  attenuation limit added).
- The voice connection does not wait for its frames (several ms each:
  waiting made the 20 ms timer late on Windows, whose timers are coarse,
  81 late ticks a minute in the demo); they leave one tick later, steadily.
- Runs with `tract-onnx` 0.23.10 and `realfft` (direct dependencies; they
  came in with `deep-filter-rt`, now removed).
- Measured in the cloud container (2 slow cores): about 4.9 ms per 10 ms
  step, noise −40 dB. On the PC: to be measured in the log
  (`voice: microphone: … ms average`), the clean-up runs on its own thread.
- Not measured yet: quality on a real voice, CPU on Windows.

### DPDFNet 16 kHz (0.19.0, experimental)

A lighter choice next to the full model: `baseline.onnx` from the same
revision (16 kHz, 2.31 M parameters, 0.36 GMACs/s against 2.42; 8,722,707
bytes, SHA-256
`371d26182aff0e1e0d31354e24c81f79cd57458f5a7dd003fc10a7ebf64255e0`), same
graph interface (`spec [1,1,161,2]`, `state [38256]`), downloaded and checked
the same way.

- Rico stays at 48 kHz: a 97-tap windowed-sinc low-pass (Blackman, cut at
  7.2 kHz) takes every third sample before the model, and the same filter
  (gain 3, zero-stuffed) brings it back after. Test: 300 Hz, 3 and 6 kHz
  pass within ±15%, 12 kHz is cut below 5%. The cleaned voice keeps nothing
  above 8 kHz. Delay: 10 ms (STFT) + 2 ms (filters).
- Measured in the cloud container: 3.05 ms per 10 ms step against 5.56 for
  the full model, so −45%, far from the 7× the MACs suggest: with a small
  model tract's fixed cost per operation weighs more. On the PC: to measure.
