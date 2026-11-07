//! Microphone and speakers (cpal, Windows only for now).
//!
//! Each device runs on its own thread, which owns the cpal stream until the
//! [`Capture`] or [`Playback`] value is dropped: dropping it stops the stream
//! and releases the device. The callbacks only copy samples to or from a
//! bounded [`SampleQueue`] in the device's own format; conversion happens in
//! the voice task (see `dsp`).

use anyhow::Result;

use super::dsp::SampleQueue;

/// Sample rate and channel count of a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceFormat {
    pub rate: u32,
    pub channels: usize,
}

/// Speakers: samples pushed to `queue` (in `format`) are played.
pub struct Playback {
    pub queue: SampleQueue,
    pub format: DeviceFormat,
    _worker: Worker,
}

/// Microphone: captured samples (in `format`) arrive in `queue`.
pub struct Capture {
    pub queue: SampleQueue,
    pub format: DeviceFormat,
    _worker: Worker,
}

// Only the Windows backend builds streams; elsewhere these stay unused.
/// Seconds of audio a device queue can hold.
#[cfg_attr(not(windows), allow(dead_code))]
const PLAYBACK_SECONDS: f32 = 0.25;
#[cfg_attr(not(windows), allow(dead_code))]
const CAPTURE_SECONDS: f32 = 0.5;

#[cfg_attr(not(windows), allow(dead_code))]
fn queue_for(format: DeviceFormat, seconds: f32) -> SampleQueue {
    SampleQueue::new((format.rate as f32 * format.channels as f32 * seconds) as usize)
}

impl Playback {
    /// Opens the default output device.
    pub fn start() -> Result<Self> {
        let (queue, format, worker) = imp::start(imp::Direction::Output)?;
        Ok(Self {
            queue,
            format,
            _worker: worker,
        })
    }
}

impl Capture {
    /// Opens the default input device. Call only after an explicit user action.
    pub fn start() -> Result<Self> {
        let (queue, format, worker) = imp::start(imp::Direction::Input)?;
        Ok(Self {
            queue,
            format,
            _worker: worker,
        })
    }
}

/// Thread holding a device stream; stops it when dropped.
#[cfg_attr(not(windows), allow(dead_code))]
struct Worker {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Closing the channel wakes the thread, which drops the stream.
        self.stop.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;
    use std::time::Duration;

    use anyhow::{anyhow, bail, Context, Result};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;

    use super::{queue_for, DeviceFormat, SampleQueue, Worker, CAPTURE_SECONDS, PLAYBACK_SECONDS};

    #[derive(Clone, Copy)]
    pub enum Direction {
        Input,
        Output,
    }

    pub fn start(direction: Direction) -> Result<(SampleQueue, DeviceFormat, Worker)> {
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(SampleQueue, DeviceFormat)>>(1);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let name = match direction {
            Direction::Input => "ferrocord-microphone",
            Direction::Output => "ferrocord-speakers",
        };
        let thread = std::thread::Builder::new().name(name.into()).spawn(move || {
            match open(direction) {
                Ok((stream, queue, format)) => {
                    if let Err(e) = stream.play() {
                        let _ = ready_tx.send(Err(anyhow!("starting the audio stream: {e}")));
                        return;
                    }
                    let _ = ready_tx.send(Ok((queue, format)));
                    // Keep the stream alive until the owner goes away.
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            }
        })?;
        let worker = Worker {
            stop: Some(stop_tx),
            thread: Some(thread),
        };
        let (queue, format) = ready_rx
            .recv_timeout(Duration::from_secs(10))
            .context("the audio device did not start")??;
        Ok((queue, format, worker))
    }

    fn open(direction: Direction) -> Result<(cpal::Stream, SampleQueue, DeviceFormat)> {
        let host = cpal::default_host();
        let (device, supported) = match direction {
            Direction::Input => {
                let d = host.default_input_device().context("no microphone found")?;
                let c = d.default_input_config()?;
                (d, c)
            }
            Direction::Output => {
                let d = host.default_output_device().context("no speakers found")?;
                let c = d.default_output_config()?;
                (d, c)
            }
        };
        let config = supported.config();
        let format = DeviceFormat {
            rate: config.sample_rate,
            channels: config.channels as usize,
        };
        if format.channels == 0 || format.rate == 0 {
            bail!("the audio device reports an invalid format");
        }
        let stream = match (direction, supported.sample_format()) {
            (Direction::Output, SampleFormat::F32) => {
                let queue = queue_for(format, PLAYBACK_SECONDS);
                let q = queue.clone();
                let s = device.build_output_stream::<f32, _, _>(
                    config,
                    move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                        q.pop_into(data);
                    },
                    |e| tracing::warn!("audio: speakers: {e}"),
                    None,
                )?;
                (s, queue)
            }
            (Direction::Output, SampleFormat::I16) => {
                let queue = queue_for(format, PLAYBACK_SECONDS);
                let q = queue.clone();
                let mut buf: Vec<f32> = Vec::new();
                let s = device.build_output_stream::<i16, _, _>(
                    config,
                    move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                        buf.resize(data.len(), 0.0);
                        q.pop_into(&mut buf);
                        for (d, s) in data.iter_mut().zip(&buf) {
                            *d = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    },
                    |e| tracing::warn!("audio: speakers: {e}"),
                    None,
                )?;
                (s, queue)
            }
            (Direction::Input, SampleFormat::F32) => {
                let queue = queue_for(format, CAPTURE_SECONDS);
                let q = queue.clone();
                let s = device.build_input_stream::<f32, _, _>(
                    config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| q.push(data),
                    |e| tracing::warn!("audio: microphone: {e}"),
                    None,
                )?;
                (s, queue)
            }
            (Direction::Input, SampleFormat::I16) => {
                let queue = queue_for(format, CAPTURE_SECONDS);
                let q = queue.clone();
                let mut buf: Vec<f32> = Vec::new();
                let s = device.build_input_stream::<i16, _, _>(
                    config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        buf.clear();
                        buf.extend(data.iter().map(|s| *s as f32 / i16::MAX as f32));
                        q.push(&buf);
                    },
                    |e| tracing::warn!("audio: microphone: {e}"),
                    None,
                )?;
                (s, queue)
            }
            (_, other) => bail!("unsupported audio sample format: {other:?}"),
        };
        let (stream, queue) = stream;
        Ok((stream, queue, format))
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{bail, Result};

    use super::{DeviceFormat, SampleQueue, Worker};

    #[derive(Clone, Copy)]
    pub enum Direction {
        Input,
        Output,
    }

    pub fn start(_: Direction) -> Result<(SampleQueue, DeviceFormat, Worker)> {
        bail!("microphone and speakers are only supported on Windows for now")
    }
}
