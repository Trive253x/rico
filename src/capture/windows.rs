//! Windows: pictures with Windows Graphics Capture (`windows-capture`),
//! sound with WASAPI process loopback (`wasapi`). Both crates wrap the
//! system's COM/WinRT interfaces; no `unsafe` code here.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use tracing::{debug, warn};
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame as WgcFrame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

use super::{Shaper, Source, SourceKind};
use crate::audio::dsp::SampleQueue;
use crate::i18n::t;
use crate::video::convert::{Frame, PixelOrder};

/// Longest window title kept (characters).
const MAX_TITLE: usize = 80;

fn monitor_of(id: u64) -> Monitor {
    Monitor::from_raw_hmonitor(std::ptr::without_provenance_mut(id as usize))
}

fn window_of(id: u64) -> Window {
    Window::from_raw_hwnd(std::ptr::without_provenance_mut(id as usize))
}

fn short(title: &str) -> String {
    let clean: String = title.chars().filter(|c| !c.is_control()).collect();
    match clean.char_indices().nth(MAX_TITLE) {
        Some((i, _)) => format!("{}…", &clean[..i]),
        None => clean,
    }
}

pub fn sources(max_windows: usize) -> Vec<Source> {
    let mut out = Vec::new();
    let primary = Monitor::primary().ok().map(|m| m.as_raw_hmonitor().addr() as u64);
    match Monitor::enumerate() {
        Ok(monitors) => {
            for (i, m) in monitors.iter().enumerate() {
                let id = m.as_raw_hmonitor().addr() as u64;
                let size = match (m.width(), m.height()) {
                    (Ok(w), Ok(h)) => format!(" · {w}×{h}"),
                    _ => String::new(),
                };
                out.push(Source {
                    id,
                    kind: SourceKind::Screen,
                    name: format!("{} {}{size}", t("Screen", "Schermo"), i + 1),
                    process_id: None,
                    primary: primary == Some(id),
                });
            }
        }
        Err(e) => warn!("capture: screens not listed: {e}"),
    }
    let me = std::process::id();
    match Window::enumerate() {
        Ok(windows) => {
            for w in windows {
                if out.len() >= max_windows + 16 {
                    break;
                }
                let Ok(pid) = w.process_id() else { continue };
                if pid == me || !w.is_valid() {
                    continue;
                }
                let title = w.title().unwrap_or_default();
                if title.trim().is_empty() {
                    continue;
                }
                out.push(Source {
                    id: w.as_raw_hwnd().addr() as u64,
                    kind: SourceKind::Window,
                    name: short(title.trim()),
                    process_id: Some(pid),
                    primary: false,
                });
            }
        }
        Err(e) => warn!("capture: windows not listed: {e}"),
    }
    out
}

/// What the capture thread needs.
struct Flags {
    shaper: Shaper,
    ended: Arc<AtomicBool>,
}

struct Handler {
    shaper: Shaper,
    ended: Arc<AtomicBool>,
}

type HandlerError = Box<dyn std::error::Error + Send + Sync>;

impl GraphicsCaptureApiHandler for Handler {
    type Flags = Flags;
    type Error = HandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            shaper: ctx.flags.shaper,
            ended: ctx.flags.ended,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut WgcFrame,
        _control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        // Windows may deliver more pictures than we send: those are dropped
        // before the copy from the graphics card, the costly part.
        if !self.shaper.wants_frame() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let mut buffer = frame.buffer()?;
        self.shaper.add_copy_time(started.elapsed());
        let (width, height, pitch) = (
            buffer.width() as usize,
            buffer.height() as usize,
            buffer.row_pitch() as usize,
        );
        let data = buffer.as_raw_buffer();
        self.shaper.push(&Frame {
            data,
            width,
            height,
            pitch,
            order: PixelOrder::Bgra,
        });
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        debug!("capture: the captured item was closed");
        self.ended.store(true, Ordering::Relaxed);
        Ok(())
    }
}

/// A running picture capture; dropping it stops the capture thread.
pub struct VideoCapture {
    control: Option<CaptureControl<Handler, HandlerError>>,
}

impl Drop for VideoCapture {
    fn drop(&mut self) {
        if let Some(c) = self.control.take() {
            if let Err(e) = c.stop() {
                debug!("capture: stopped with: {e}");
            }
        }
    }
}

impl VideoCapture {
    pub fn start(source: &Source, shaper: Shaper, ended: Arc<AtomicBool>) -> Result<Self> {
        let interval = shaper.min_interval();
        let flags = Flags { shaper, ended };
        // Without the yellow border where Windows lets us (Rico shows
        // "Live" instead); elsewhere the system's default.
        let border = if GraphicsCaptureApi::is_border_settings_supported().unwrap_or(false) {
            DrawBorderSettings::WithoutBorder
        } else {
            DrawBorderSettings::Default
        };
        let update = if GraphicsCaptureApi::is_minimum_update_interval_supported().unwrap_or(false) {
            MinimumUpdateIntervalSettings::Custom(interval)
        } else {
            MinimumUpdateIntervalSettings::Default
        };
        let control = match source.kind {
            SourceKind::Screen => Handler::start_free_threaded(Settings::new(
                monitor_of(source.id),
                CursorCaptureSettings::Default,
                border,
                SecondaryWindowSettings::Default,
                update,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                flags,
            )),
            SourceKind::Window => {
                let window = window_of(source.id);
                if !window.is_valid() {
                    return Err(anyhow!(t(
                        "That window is no longer open.",
                        "Quella finestra non è più aperta."
                    )));
                }
                Handler::start_free_threaded(Settings::new(
                    window,
                    CursorCaptureSettings::Default,
                    border,
                    SecondaryWindowSettings::Default,
                    update,
                    DirtyRegionSettings::Default,
                    ColorFormat::Bgra8,
                    flags,
                ))
            }
            SourceKind::TestPattern => return Err(anyhow!("not a system source")),
        }
        .map_err(|e| anyhow!("{e}"))
        .context(t(
            "screen capture did not start",
            "la cattura dello schermo non è partita",
        ))?;
        debug!("capture: started ({:?}, border {:?})", source.kind, border);
        Ok(Self {
            control: Some(control),
        })
    }
}

/// Sound of a program (with its child processes), or of every program but
/// Rico, as 48 kHz stereo floats.
pub struct Loopback {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Loopback {
    pub fn start(source: &Source, queue: SampleQueue) -> Result<Self> {
        // A window: its program's sound. A screen: everything except us.
        let (pid, include_tree) = match (source.kind, source.process_id) {
            (SourceKind::Window, Some(pid)) => (pid, true),
            _ => (std::process::id(), false),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("capture-sound".into())
            .spawn(move || {
                if let Err(e) = loopback(pid, include_tree, &queue, &s, &ready_tx) {
                    let _ = ready_tx.try_send(Err(e.to_string()));
                    warn!("capture: sound stopped: {e}");
                }
            })?;
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(e)) => Err(anyhow!(e)),
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err(anyhow!(t(
                    "the sound capture did not answer",
                    "la cattura dell'audio non ha risposto"
                )))
            }
        }
    }
}

fn loopback(
    pid: u32,
    include_tree: bool,
    queue: &SampleQueue,
    stop: &AtomicBool,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
) -> Result<(), wasapi::WasapiError> {
    let _ = wasapi::initialize_mta();
    let format = wasapi::WaveFormat::new(32, 32, wasapi::SampleType::Float, 48_000, 2, None);
    let mut client = wasapi::AudioClient::new_application_loopback_client(pid, include_tree)?;
    let mode = wasapi::StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: 200_000,
    };
    client.initialize_client(&format, wasapi::Direction::Capture, mode, None)?;
    let event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    client.start_stream()?;
    let _ = ready.try_send(Ok(()));
    let mut bytes: VecDeque<u8> = VecDeque::new();
    let mut samples: Vec<f32> = Vec::with_capacity(4096);
    while !stop.load(Ordering::Relaxed) {
        // No event for a while just means silence.
        if event.wait_for_event(100).is_err() {
            continue;
        }
        capture.read_from_device_to_deque(&mut bytes)?;
        samples.clear();
        while bytes.len() >= 4 {
            let b = [bytes[0], bytes[1], bytes[2], bytes[3]];
            bytes.drain(..4);
            let s = f32::from_le_bytes(b);
            samples.push(if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 });
        }
        if !samples.is_empty() {
            queue.push(&samples);
        }
    }
    let _ = client.stop_stream();
    Ok(())
}
