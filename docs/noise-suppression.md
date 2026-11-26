# Noise suppression

How Rico cleans the microphone before sending it, which noise suppressors
can be chosen, how they run and why they are these. Code: `src/audio/process.rs`
(the clean-up and the choice) and `src/audio/dpdfnet.rs` (the DPDFNet model).

## The choices

Settings › Voice & video › Voice processing › Noise suppression:

| Choice | Removes | Cost | Delay | Comes with |
|---|---|---|---|---|
| **Off** | nothing | none | none | — |
| **Standard** (default) | steady noise: fan, hiss, air conditioning | almost no CPU | none | the program (`sonora`, WebRTC's audio processing) |
| **RNNoise** | also keyboard and sudden noise | about 0.6 % of a core | none | the program (`nnnoiseless`) |
| **DPDFNet** (its 16 kHz model) | the strongest; no sound above 8 kHz in the cleaned voice | about 9 % of a core (owner's PC) | 12 ms | downloaded once when chosen (8.7 MB) |

- **Standard is the default**: nothing changes for who updates until they
  choose something else.
- **Reading the settings** (`Noise`'s `Deserialize`):
  - the old switch, `true`/`false`, is read as Standard/Off;
  - a saved `deep_filter` (removed in 0.19.0) is read as RNNoise, the
    nearest choice that needs no download;
  - a saved `dpdf_net` (the 48 kHz model, removed in 0.20.0) is read as
    DPDFNet's 16 kHz model if that is already downloaded, otherwise as
    RNNoise; the user is told once and the old model's file is deleted;
  - DPDFNet is saved as `dpdf_net_light`, its name from before;
  - a missing or unknown value is read as Standard, never a weaker
    setting by surprise.
- **During a call** the choice can be changed and applies at once, to
  compare them.
- **Echo cancellation and automatic gain** are separate switches and work
  with every choice.
- **If the chosen suppressor fails** (model missing, corrupt or not
  loadable, a panic in the loader, an error while running), Standard runs
  instead and the user is told once. The microphone is never left without
  a filter silently.
- **If DPDFNet is too slow** on this computer (busy CPU), RNNoise takes over
  for the rest of the call, with a notice in the window (see below).
- Everything runs on the computer: no audio leaves it. The only network
  use is the one-time download of a DPDFNet model.

## How it runs

- **Thread**: the clean-up has a thread of its own, so the voice
  connection and the interface never wait for it. Frames reach it through
  a bounded queue (at most 200 ms); beyond that the oldest are dropped
  rather than queued without limit.
- **Order**: echo canceller → noise suppressor → automatic gain → soft
  limiter. The echo canceller hears what the speakers are about to play
  (`Processor::render`) before cleaning the microphone (`Processor::capture`).
- **Format**: everything is 48 kHz mono, 10 ms at a time. WebRTC's own
  noise suppression runs only for Standard (or as the fallback).
- **When it runs**: only while the microphone is on. A DPDFNet model is read
  and checked on its own thread when it is chosen. Until it is ready, echo
  cancellation and gain still run. The model is dropped when the call ends
  or another choice is made.
- **Waiting for the clean-up**: the voice connection waits up to 12 ms for
  the cleaned frames; a late one leaves with the next tick. (The removed
  48 kHz model took several ms a step and did not wait at all: waiting made
  the 20 ms timer late on Windows, 81 late ticks a minute in the demo.)
- **When DPDFNet is too slow**: the clean-up thread measures every 20 ms
  frame. If in 2 seconds it took more than 12 ms a frame on average (60 % of
  the time there is), three times in a row (6 s, so a short spike does not
  count), RNNoise replaces DPDFNet for the rest of the call. The window
  shows a notice with three answers: OK, "Always keep DPDFNet" (turns on the
  switch below and restarts DPDFNet at once) and "Use RNNoise from now on".
  The next call tries DPDFNet again.
- **"Always use the chosen model"** (Settings › Voice & video, shown with
  DPDFNet; off by default): DPDFNet is never replaced, even if the others
  hear gaps.
- **Timings in the log**: `voice: microphone: … ms average`, with
  `RICO_LOG=warn,rico=debug`.

## RNNoise

`nnnoiseless` 0.5.2 (BSD-3-Clause), a pure-Rust port of RNNoise, with
`default-features = false`.

- **Dependencies**: it adds only `easyfft` and the FFT crates it uses. No
  build script and no native code; its C API is behind a feature that stays
  off.
- **`unsafe`**: one block, reinterpreting `&[u8]` weights as `&[i8]`. It is
  sound.
- **Cost**: about +1.2 MB to the program, about 0.6 % of one core, no extra
  delay.
- **Samples**: it expects them in the `i16` range as `f32`, so Rico scales
  by 32767 in and out.
- **Maintenance**: since 2020, more than a million downloads, last release
  2025-12-18.

## DPDFNet

DPDFNet (Ceva, arXiv 2512.16420) is DeepFilterNet2 with dual-path blocks in
the encoder, trained with a loss against over-attenuation. On the authors'
own multilingual low-SNR set it scores above DeepFilterNet2 and 3 on almost
every metric; there is no independent comparison yet.

### The model

From Hugging Face `Ceva-IP/DPDFNet` at revision
`dd6818d00f50c836fed43a6243ebe49116de5964` (Apache-2.0; code on GitHub
`ceva-ip/DPDFNet`):

| Model | File | Size | SHA-256 | Parameters | Compute |
|---|---|---|---|---|---|
| DPDFNet 16 kHz | `baseline.onnx` | 8,722,707 bytes | `371d26182aff0e1e0d31354e24c81f79cd57458f5a7dd003fc10a7ebf64255e0` | 2.31 M | 0.36 GMACs/s |
| *removed in 0.20.0:* DPDFNet 48 kHz | `dpdfnet2_48khz_hr` | 10,493,337 bytes | `7f0575a5…` | 2.58 M | 2.42 GMACs/s |

### Download

- The model is **not in the installer**: it is downloaded once, when
  chosen (owner's choice, so the repository and the installer stay small).
- The download goes through `media::fetch_model`:
  - HTTPS, `huggingface.co` and `*.hf.co` only;
  - redirects followed by hand, each one checked;
  - a size limit.
- The file is checked against its SHA-256 before it is saved and again at
  every load. In the demo nothing is downloaded.

### The graph and the STFT around it

- **Interface**: `spec [1,1,161,2]` and `state_in [38256]` in, `spec_e`
  and `state_out` out.
  The features, the normalisation and the deep filter are inside the graph.
- **STFT**: Rico does the causal STFT around the graph as upstream's
  `StreamEnhancer`: a Vorbis window of 320 samples (at 16 kHz), hop 160, overlap-add.
  The initial state comes from the model's metadata. The STFT adds 10 ms of
  delay.
- **Output**: the model's own output is used unchanged, as upstream (owner's
  choice: no attenuation limit added).
- **48 kHz around it**: Rico stays at 48 kHz.
  - A 97-tap windowed-sinc low-pass (Blackman, cut at 7.2 kHz) takes every
    third sample before the model.
  - The same filter, with gain 3 and zero-stuffing, brings the signal back
    up after it.
  - Test: 300 Hz, 3 kHz and 6 kHz pass within ±15 %, 12 kHz is cut below
    5 %.
  - The filters add 2 ms of delay.
- **Inference**: `tract-onnx` 0.23.10 (Sonos) and `realfft`, direct
  dependencies. `tract-linalg` compiles optimized kernels with `cc` (native
  code).

### Measures

| Where | DPDFNet 48 kHz (removed) | DPDFNet 16 kHz |
|---|---|---|
| Cloud container (2 slow cores) | 5.56 ms per 10 ms step | 3.05 ms per 10 ms step |
| Owner's PC, release build, demo (23 November 2026) | 4.5 ms per 20 ms frame, about 22 % of a core | 1.8–1.9 ms per 20 ms frame, about 9 % of a core |

The 16 kHz model needs 7× fewer MACs but saved only about 45 %: with a small
model tract's fixed cost per operation weighs more.

**Not measured yet**: the quality on a real voice for every choice, and the
switch to RNNoise on the owner's PC under load.

## History and decisions

- **0.18 and earlier**: one switch, WebRTC's suppression (`sonora`,
  `NoiseSuppressionLevel::High`). The owner found it weak on Discord: it
  removes steady noise but lets most sudden noise through (keyboard, clicks,
  dogs, dishes, voices in the background).
- **Krisp**, which Discord uses, was not possible: it is a closed SDK,
  available only under a commercial contract.
- **2026-11-20, dependency checks** before adding anything (AGENTS.md):
  - RNNoise passed;
  - the official DeepFilterNet crate on crates.io (`deep_filter` 0.2.5,
    2022) has no model inference, and upstream is usable only as a git
    dependency, which `deny.toml` refuses;
  - `deep-filter-rt` 0.2.0 was checked: model byte for byte upstream's,
    weights license MIT/Apache-2.0, code reviewed. Its risks were noted: a
    new crate with one maintainer, 55 more crates, a bigger installer.
  - With both, `cargo deny check` and `cargo audit` gave the same findings
    as before.
- **2026-11-21, 0.19.0 candidates**: the owner chose a choice in the
  settings with RNNoise and DeepFilterNet (`deep-filter-rt` pinned to
  `=0.2.0`).
- **DeepFilterNet removed before the 0.19.0 release**. With its defaults,
  where it hears no voice it writes exact zeros, and soft parts of words fell
  into those holes ("casa" heard as "c-asa", above all on PlayStation 5;
  RNNoise did not do it).
  - First fix: an attenuation limit of 30 dB and a
    "noise only" threshold of −15 dB.
  - Then it was removed before the release, and DPDFNet, trained against
    over-attenuation, took its place. It had shipped only in candidates.
  - `deep-filter-rt` is no longer a dependency; `tract-onnx` stayed for
    DPDFNet.
- **0.19.0**: DPDFNet added as the strong choice, then DPDFNet 16 kHz as the
  lighter one.
- **0.20.0**: under load (compiling Rico during a call, 23 November 2026)
  the 48 kHz model slowed to 17–19 ms per 20 ms frame: frames were dropped
  and the microphone buffer overran. The owner chose to remove the 48 kHz
  model, keep only the 16 kHz one, switch to RNNoise by itself when the
  clean-up stays too slow, tell the user with a notice, and add a switch to
  keep DPDFNet anyway.
