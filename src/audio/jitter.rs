//! Jitter buffer and mixer for received voice.
//!
//! Packets of each speaker are kept by RTP sequence number and decoded only
//! when they are due, so that:
//!
//! - packets arriving out of order are played in order;
//! - a lost packet is rebuilt from the next one (Opus in-band FEC) or
//!   concealed (Opus PLC) instead of leaving a hole, which would be heard as
//!   a click;
//! - playback of a speaker starts with a small cushion ([`TARGET`]) that
//!   absorbs network jitter, and keeps going for a moment with concealment if
//!   the network stalls in the middle of a word;
//! - a backlog (a burst after a stall) is cut back to the cushion instead of
//!   piling up delay.
//!
//! The mixer is pulled by the speakers' clock (see `voice::Media::play`):
//! audio is produced when the output device needs it, not on a timer.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, Instant};

use super::codec::{packet_samples, Decoder};
use super::{CHANNELS, FRAME_LEN, FRAME_SAMPLES};

/// Audio buffered for a speaker before playback starts (100 ms): the
/// network jitter it can absorb.
pub const TARGET: usize = 5 * FRAME_SAMPLES;
/// The speakers are kept this far ahead (40 ms, two frames).
///
/// Enough for a late timer tick on Windows (up to ~31 ms), and well under
/// [`TARGET`]. Filling them further ahead would empty the jitter buffer into them: a
/// packet a little late would then find its turn gone (concealed, and then
/// dropped as late), which is heard as stuttering.
pub const DEVICE_AHEAD_MS: u64 = 40;
/// A few packets that never reach [`TARGET`] (a very short sound) are
/// played anyway after this long.
const START_AFTER: Duration = Duration::from_millis(100);
/// Beyond this much buffered audio (300 ms) the oldest is skipped, back to
/// [`TARGET`].
const MAX_BUFFERED: usize = 15 * FRAME_SAMPLES;
/// Packets kept per speaker at most (also bounds memory).
const MAX_PACKETS: usize = 64;
/// While speaking, missing audio is concealed for at most this long (60 ms)
/// before playback stops and waits for a new cushion.
const MAX_CONCEAL: usize = 3 * FRAME_SAMPLES;
/// A jump forward of more than this many packets is a new start, not a loss.
const MAX_GAP: u64 = 50;
/// Most speakers mixed at once.
const MAX_SPEAKERS: usize = 64;
/// A speaker quiet for this long is forgotten (decoder state included).
const SPEAKER_IDLE: Duration = Duration::from_secs(30);

/// Opus silence frame: what clients send for a moment after they stop.
const SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

/// What happened to received audio, for the statistics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JitterStats {
    /// Lost packets rebuilt from the next packet's FEC data.
    pub recovered: u64,
    /// Missing audio replaced by concealment (in 20 ms frames).
    pub concealed: u64,
    /// Packets arriving after their turn (dropped).
    pub late: u64,
    /// Packets skipped to cut a backlog, or beyond the limits.
    pub skipped: u64,
    /// Times a speaker ran dry while speaking (playback stopped mid-word).
    pub underruns: u64,
}

struct Packet {
    data: Vec<u8>,
    /// Samples per channel.
    samples: usize,
}

/// One speaker (one SSRC).
struct Speaker {
    decoder: Decoder,
    packets: BTreeMap<u64, Packet>,
    /// Last extended sequence number seen (to unwrap the 16-bit one).
    last_ext: Option<u64>,
    /// Next sequence number to play.
    next: Option<u64>,
    /// Decoded samples (interleaved stereo) not played yet.
    pcm: VecDeque<f32>,
    playing: bool,
    /// The last packet played was a silence frame (the speaker stopped).
    silent: bool,
    /// Samples per channel of the last packet (the size of a lost one).
    last_samples: usize,
    /// Concealed samples since the last real packet.
    concealed_run: usize,
    last_push: Instant,
    /// When the oldest packet waiting for playback to start arrived.
    waiting_since: Option<Instant>,
}

impl Speaker {
    fn new(decoder: Decoder) -> Self {
        Self {
            decoder,
            packets: BTreeMap::new(),
            last_ext: None,
            next: None,
            pcm: VecDeque::new(),
            playing: false,
            silent: true,
            last_samples: FRAME_SAMPLES,
            concealed_run: 0,
            last_push: Instant::now(),
            waiting_since: None,
        }
    }

    /// 16-bit sequence number → 64-bit, following wraps both ways.
    fn extend(&mut self, seq: u16) -> u64 {
        let ext = match self.last_ext {
            None => (1 << 16) + u64::from(seq),
            Some(last) => {
                let delta = i64::from(seq.wrapping_sub(last as u16) as i16);
                (last as i64 + delta).max(0) as u64
            }
        };
        if self.last_ext.is_none_or(|last| ext > last) {
            self.last_ext = Some(ext);
        }
        ext
    }

    fn buffered(&self) -> usize {
        self.packets.values().map(|p| p.samples).sum::<usize>() + self.pcm.len() / CHANNELS
    }

    fn push(&mut self, seq: u16, data: Vec<u8>, stats: &mut JitterStats) {
        self.last_push = Instant::now();
        let Some(samples) = packet_samples(&data) else {
            stats.skipped += 1;
            return;
        };
        let ext = self.extend(seq);
        if let Some(next) = self.next {
            if ext < next {
                // Its turn has passed (played, recovered or concealed).
                stats.late += 1;
                return;
            }
            if ext > next + MAX_GAP && self.packets.is_empty() {
                // A long pause or a restart: start over from here.
                self.next = None;
                self.playing = false;
            }
        }
        if self.packets.len() >= MAX_PACKETS {
            if let Some((&first, _)) = self.packets.iter().next() {
                if first > ext {
                    stats.skipped += 1;
                    return;
                }
                self.packets.remove(&first);
                stats.skipped += 1;
            }
        }
        if !self.playing && self.waiting_since.is_none() {
            self.waiting_since = Some(Instant::now());
        }
        self.packets.insert(ext, Packet { data, samples });
    }

    /// Up to 20 ms of this speaker's audio, added to `mix`. `false` if the
    /// speaker is not playing.
    fn pull(&mut self, mix: &mut [f32], gain: f32, stats: &mut JitterStats) -> bool {
        if !self.playing {
            let waited = self.waiting_since.is_some_and(|t| t.elapsed() >= START_AFTER);
            if self.packets.is_empty() || (self.buffered() < TARGET && !waited) {
                return false;
            }
            self.playing = true;
            self.waiting_since = None;
            self.concealed_run = 0;
            let first = *self.packets.keys().next().unwrap_or(&0);
            if self.next.is_none_or(|n| n < first) {
                self.next = Some(first);
            }
        }
        if self.buffered() > MAX_BUFFERED {
            self.cut_backlog(stats);
        }
        while self.pcm.len() < FRAME_LEN {
            if !self.refill(stats) {
                break;
            }
        }
        if self.pcm.is_empty() {
            return false;
        }
        let n = self.pcm.len().min(FRAME_LEN);
        if n < FRAME_LEN && !self.silent {
            // Ran dry in the middle of speech: fade the last samples out
            // instead of cutting them (a cut is heard as a click).
            stats.underruns += 1;
        }
        for (i, (m, s)) in mix.iter_mut().zip(self.pcm.drain(..n)).enumerate() {
            let fade = if n < FRAME_LEN {
                1.0 - (i / CHANNELS) as f32 / (n / CHANNELS) as f32
            } else {
                1.0
            };
            *m += s * fade * gain;
        }
        true
    }

    /// Decodes the next due packet into `pcm`: the packet itself, or a
    /// replacement if it is missing. `false` when playback stops.
    fn refill(&mut self, stats: &mut JitterStats) -> bool {
        let Some(next) = self.next else {
            self.playing = false;
            return false;
        };
        if let Some(p) = self.packets.remove(&next) {
            self.next = Some(next + 1);
            self.last_samples = p.samples;
            self.concealed_run = 0;
            self.silent = p.data == SILENCE;
            match self.decoder.decode(&p.data) {
                Ok(pcm) => self.pcm.extend(pcm),
                // A broken packet: conceal it like a lost one.
                Err(_) => self.conceal(stats),
            }
            return true;
        }
        if self.packets.is_empty() {
            // Nothing more yet. After a silence frame the speaker simply
            // stopped; in the middle of speech, conceal a little while
            // waiting for the network.
            if self.silent || self.concealed_run >= MAX_CONCEAL {
                self.playing = false;
                return false;
            }
            self.next = Some(next + 1);
            self.conceal(stats);
            return true;
        }
        // `next` is missing but later packets are here: lost (or very late).
        self.next = Some(next + 1);
        let recovered = match self.packets.get(&(next + 1)) {
            Some(after) if !self.silent => self.decoder.recover(&after.data, self.last_samples).ok(),
            _ => None,
        };
        match recovered {
            Some(pcm) => {
                stats.recovered += 1;
                self.pcm.extend(pcm);
            }
            None if self.silent => {}
            None => self.conceal(stats),
        }
        true
    }

    fn conceal(&mut self, stats: &mut JitterStats) {
        stats.concealed += 1;
        self.concealed_run += self.last_samples;
        match self.decoder.conceal_samples(self.last_samples) {
            Ok(pcm) => self.pcm.extend(pcm),
            Err(_) => self
                .pcm
                .extend(std::iter::repeat_n(0.0, self.last_samples * CHANNELS)),
        }
    }

    /// Too much buffered (a burst after a stall): skip the oldest packets,
    /// back to the cushion.
    fn cut_backlog(&mut self, stats: &mut JitterStats) {
        self.pcm.clear();
        while self.buffered() > TARGET {
            let Some((&first, _)) = self.packets.iter().next() else {
                break;
            };
            self.packets.remove(&first);
            stats.skipped += 1;
            self.next = Some(first + 1);
        }
        if let Some((&first, _)) = self.packets.iter().next() {
            self.next = Some(first);
        }
    }
}

/// Received audio of everyone in the call, mixed 20 ms at a time.
#[derive(Default)]
pub struct Mixer {
    speakers: HashMap<u32, Speaker>,
    /// Gain of single speakers (per-person volume); missing: 1.0.
    gains: HashMap<u32, f32>,
    pub stats: JitterStats,
}

impl Mixer {
    /// Queues a received Opus packet (silence frames included: they tell when
    /// someone stopped). `false` if it was not accepted.
    pub fn push(&mut self, ssrc: u32, seq: u16, packet: &[u8]) -> bool {
        if !self.speakers.contains_key(&ssrc) {
            if packet == SILENCE || self.speakers.len() >= MAX_SPEAKERS {
                return false;
            }
            let Ok(decoder) = Decoder::new() else { return false };
            self.speakers.insert(ssrc, Speaker::new(decoder));
        }
        let Some(speaker) = self.speakers.get_mut(&ssrc) else {
            return false;
        };
        speaker.push(seq, packet.to_vec(), &mut self.stats);
        true
    }

    pub fn remove(&mut self, ssrc: u32) {
        self.speakers.remove(&ssrc);
        self.gains.remove(&ssrc);
    }

    /// Volume of one speaker (0.0 silent, 1.0 as received).
    pub fn set_gain(&mut self, ssrc: u32, gain: f32) {
        if gain == 1.0 || !gain.is_finite() {
            self.gains.remove(&ssrc);
        } else if self.gains.len() < MAX_SPEAKERS || self.gains.contains_key(&ssrc) {
            self.gains.insert(ssrc, gain.max(0.0));
        }
    }

    /// Forgets everyone (sound turned off); the statistics stay.
    pub fn clear(&mut self) {
        self.speakers.clear();
    }

    /// The next 20 ms of mixed audio, or `None` if nobody is playing.
    pub fn mix(&mut self) -> Option<Vec<f32>> {
        let now = Instant::now();
        self.speakers.retain(|_, s| {
            s.playing || !s.packets.is_empty() || now.duration_since(s.last_push) < SPEAKER_IDLE
        });
        let mut out = vec![0.0; FRAME_LEN];
        let mut any = false;
        for (ssrc, speaker) in &mut self.speakers {
            let gain = self.gains.get(ssrc).copied().unwrap_or(1.0);
            any |= speaker.pull(&mut out, gain, &mut self.stats);
        }
        if !any {
            return None;
        }
        for s in &mut out {
            *s = s.clamp(-1.0, 1.0);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::codec::Encoder;

    fn tone(frame: usize) -> Vec<f32> {
        (0..FRAME_SAMPLES)
            .flat_map(|i| {
                let t = (frame * FRAME_SAMPLES + i) as f32 / 48_000.0;
                let s = (t * 330.0 * std::f32::consts::TAU).sin() * 0.3;
                [s, s]
            })
            .collect()
    }

    fn packets(n: usize) -> Vec<Vec<u8>> {
        let mut enc = Encoder::new().unwrap();
        (0..n).map(|f| enc.encode(&tone(f)).unwrap()).collect()
    }

    fn rms(pcm: &[f32]) -> f32 {
        (pcm.iter().map(|s| s * s).sum::<f32>() / pcm.len() as f32).sqrt()
    }

    #[test]
    fn waits_for_the_cushion_then_plays_in_order() {
        let p = packets(6);
        let mut m = Mixer::default();
        m.push(7, 100, &p[0]);
        m.push(7, 101, &p[1]);
        assert!(m.mix().is_none(), "40 ms are not enough to start");
        // Out of order: 103 before 102; then 100 ms are there.
        m.push(7, 103, &p[3]);
        m.push(7, 102, &p[2]);
        m.push(7, 104, &p[4]);
        for _ in 0..5 {
            assert_eq!(m.mix().unwrap().len(), FRAME_LEN);
        }
        assert_eq!(m.stats, JitterStats::default(), "nothing lost, nothing late");
    }

    /// The speakers pull a frame whenever their queue is under
    /// [`DEVICE_AHEAD_MS`] (checked every ~16 ms, sometimes late), packets
    /// arrive every 20 ms with up to 60 ms of jitter: nothing should be
    /// concealed or late. With the speakers filled as far ahead as the cushion
    /// (the 0.7.0 behaviour) about one packet in eight was concealed.
    #[test]
    fn jitter_is_absorbed_while_the_speakers_pull() {
        let n = 400usize;
        let p = packets(n);
        let mut m = Mixer::default();
        // Arrival time (ms) of each packet: 20 ms apart plus 0..60 ms of jitter.
        let mut seed = 12345u64;
        let mut jitter = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) % 60
        };
        let mut arrivals: Vec<(u64, usize)> = (0..n).map(|i| (i as u64 * 20 + jitter(), i)).collect();
        arrivals.sort_unstable();
        let (mut now, mut queued_ms, mut next_arrival, mut tick) = (0u64, 0i64, 0usize, 0u64);
        while now < n as u64 * 20 + 200 {
            tick += 1;
            let step = if tick % 17 == 0 { 31 } else { 16 };
            now += step;
            queued_ms = (queued_ms - step as i64).max(0);
            while next_arrival < n && arrivals[next_arrival].0 <= now {
                let i = arrivals[next_arrival].1;
                m.push(9, i as u16, &p[i]);
                next_arrival += 1;
            }
            while queued_ms < DEVICE_AHEAD_MS as i64 {
                if m.mix().is_none() {
                    break;
                }
                queued_ms += 20;
            }
        }
        assert_eq!(m.stats.late, 0, "{:?}", m.stats);
        // Only the tail after the last packet (no silence frames) is concealed.
        assert!(m.stats.concealed <= 3, "{:?}", m.stats);
    }

    #[test]
    fn a_very_short_sound_still_plays() {
        let p = packets(1);
        let mut m = Mixer::default();
        m.push(8, 0, &p[0]);
        assert!(m.mix().is_none());
        m.speakers.get_mut(&8).unwrap().waiting_since = Instant::now().checked_sub(START_AFTER);
        assert!(m.mix().is_some());
    }

    #[test]
    fn a_lost_packet_is_rebuilt_not_a_hole() {
        let p = packets(12);
        let mut m = Mixer::default();
        for (i, packet) in p.iter().enumerate() {
            if i != 6 {
                m.push(1, i as u16, packet);
            }
        }
        let mut levels = Vec::new();
        while let Some(frame) = m.mix() {
            levels.push(rms(&frame));
            if levels.len() > 20 {
                break;
            }
        }
        // 12 frames of tone (one rebuilt), then concealment fades out.
        assert!(levels.len() >= 12, "{levels:?}");
        assert!(
            levels[1..12].iter().all(|l| *l > 0.05),
            "no silent hole: {levels:?}"
        );
        assert_eq!(m.stats.recovered, 1);
        // Then 60 ms of concealment after the last packet (no silence frames).
        assert_eq!(m.stats.concealed, 3);
        // The packet arriving after its turn is dropped.
        m.push(1, 6, &p[6]);
        assert_eq!(m.stats.late, 1);
    }

    #[test]
    fn silence_frames_end_playback_cleanly() {
        let p = packets(4);
        let mut m = Mixer::default();
        for (i, packet) in p.iter().enumerate() {
            m.push(2, i as u16, packet);
        }
        for i in 4..9 {
            m.push(2, i, &SILENCE);
        }
        let mut frames = 0;
        while m.mix().is_some() {
            frames += 1;
            assert!(frames < 20);
        }
        assert_eq!(frames, 9);
        assert_eq!(m.stats.concealed, 0);
        assert_eq!(m.stats.underruns, 0);
        // A new sentence after a long pause starts again with a cushion.
        let q = packets(TARGET / FRAME_SAMPLES);
        for (i, packet) in q.iter().enumerate() {
            m.push(2, 500 + i as u16, packet);
        }
        assert!(m.mix().is_some());
        assert_eq!(m.stats.late, 0);
    }

    #[test]
    fn stall_is_concealed_then_backlog_is_cut() {
        let p = packets(40);
        let mut m = Mixer::default();
        let cushion = TARGET / FRAME_SAMPLES;
        for (i, packet) in p[..cushion].iter().enumerate() {
            m.push(3, i as u16, packet);
        }
        for _ in 0..cushion {
            m.mix().unwrap();
        }
        // The network stalls: a little concealment, then playback stops.
        let mut concealed = 0;
        while m.mix().is_some() {
            concealed += 1;
            assert!(concealed < 10);
        }
        assert_eq!(concealed, MAX_CONCEAL / FRAME_SAMPLES);
        assert_eq!(m.stats.underruns, 0, "faded out, not cut");
        // Everything arrives at once: the delay is cut back to the cushion.
        for (i, packet) in p[cushion..].iter().enumerate() {
            m.push(3, (cushion + i) as u16, packet);
        }
        m.mix().unwrap();
        let buffered = m.speakers[&3].buffered();
        assert!(buffered <= TARGET, "{buffered}");
        assert!(m.stats.late > 0 && m.stats.skipped > 0);
    }

    #[test]
    fn sequence_wrap_and_limits() {
        let p = packets(5);
        let mut m = Mixer::default();
        for (i, seq) in [65533u16, 65534, 65535, 0, 1].into_iter().enumerate() {
            m.push(4, seq, &p[i]);
        }
        for _ in 0..5 {
            m.mix().unwrap();
        }
        assert_eq!(m.stats, JitterStats::default());
        // Garbage and silence from unknown speakers are not accepted.
        assert!(!m.push(5, 0, &SILENCE));
        m.push(4, 2, &[]);
        assert_eq!(m.stats.skipped, 1);
        for ssrc in 100..(100 + MAX_SPEAKERS as u32) {
            m.push(ssrc, 0, &p[0]);
        }
        assert!(m.speakers.len() <= MAX_SPEAKERS);
    }

    #[test]
    fn a_speaker_can_be_turned_down_or_off() {
        let p = packets(10);
        let level = |gain: f32| {
            let mut m = Mixer::default();
            m.set_gain(7, gain);
            for (i, packet) in p.iter().enumerate() {
                m.push(7, 100 + i as u16, packet);
            }
            let mut all = Vec::new();
            for _ in 0..8 {
                if let Some(x) = m.mix() {
                    all.extend(x);
                }
            }
            rms(&all)
        };
        let full = level(1.0);
        assert!(full > 0.05, "{full}");
        assert!((level(0.5) / full - 0.5).abs() < 0.05);
        assert_eq!(level(0.0), 0.0);
    }
}
