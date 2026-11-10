//! Minimal reader for the short MP4 clips Discord serves for GIFs (Tenor,
//! Giphy): the first H.264 video track, its parameter sets and where each
//! sample is in the file. Enough to decode the clip with OpenH264 and show
//! it as an animation, without asking anything of the GIF site (the file
//! comes from Discord's media proxy).
//!
//! The file comes from the network: every size and offset is checked, the
//! number of samples and boxes is bounded, and nothing here panics on bad
//! input (it returns an error).

use anyhow::{bail, Context, Result};

/// Most samples (frames) read from a clip.
pub const MAX_SAMPLES: usize = 2000;
/// Most boxes looked at, against files made of millions of tiny boxes.
const MAX_BOXES: usize = 10_000;
/// Nesting deeper than this is not followed.
const MAX_DEPTH: usize = 8;

/// The video track of a clip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub width: u32,
    pub height: u32,
    /// Ticks per second of the sample durations.
    pub timescale: u32,
    /// Bytes of the length in front of each NAL unit (1, 2 or 4).
    pub nal_length: usize,
    /// SPS and PPS, without start codes.
    pub params: Vec<Vec<u8>>,
    pub samples: Vec<Sample>,
}

/// A frame of the clip, in decoding order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub offset: usize,
    pub size: usize,
    /// Duration in `timescale` ticks.
    pub duration: u32,
}

/// The bytes are an MP4 (or QuickTime) file.
pub fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() > 12 && &bytes[4..8] == b"ftyp"
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).context("size overflow")?;
        let out = self.data.get(self.pos..end).context("truncated box")?;
        self.pos = end;
        Ok(out)
    }

    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }
}

/// The boxes directly inside `data`: (type, content).
fn boxes<'a>(data: &'a [u8], counter: &mut usize) -> Result<Vec<([u8; 4], &'a [u8])>> {
    let mut out = Vec::new();
    let mut r = Reader::new(data);
    while r.pos + 8 <= data.len() {
        *counter += 1;
        if *counter > MAX_BOXES {
            bail!("too many boxes");
        }
        let start = r.pos;
        let size32 = r.u32()?;
        let mut kind = [0u8; 4];
        kind.copy_from_slice(r.take(4)?);
        let size = match size32 {
            0 => data.len() - start,
            1 => usize::try_from(r.u64()?).context("box too large")?,
            n => n as usize,
        };
        let header = r.pos - start;
        if size < header {
            bail!("box size smaller than its header");
        }
        let end = start.checked_add(size).context("size overflow")?;
        let body = data
            .get(start + header..end)
            .context("box past the end of the file")?;
        out.push((kind, body));
        r.pos = end;
    }
    Ok(out)
}

fn child<'a>(list: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Option<&'a [u8]> {
    list.iter().find(|(k, _)| k == kind).map(|(_, b)| *b)
}

/// Reads the first H.264 video track of an MP4 file.
pub fn parse(bytes: &[u8]) -> Result<Clip> {
    if !is_mp4(bytes) {
        bail!("not an MP4 file");
    }
    let mut counter = 0;
    let top = boxes(bytes, &mut counter)?;
    let moov = child(&top, b"moov").context("no moov box")?;
    let moov = boxes(moov, &mut counter)?;
    for (kind, trak) in &moov {
        if kind != b"trak" {
            continue;
        }
        if let Some(clip) = video_track(bytes, trak, &mut counter, 2)? {
            return Ok(clip);
        }
    }
    bail!("no H.264 video track")
}

fn video_track(file: &[u8], trak: &[u8], counter: &mut usize, depth: usize) -> Result<Option<Clip>> {
    if depth > MAX_DEPTH {
        bail!("boxes nested too deep");
    }
    let trak = boxes(trak, counter)?;
    let Some(mdia) = child(&trak, b"mdia") else {
        return Ok(None);
    };
    let mdia = boxes(mdia, counter)?;
    // Handler: only the video track.
    let hdlr = child(&mdia, b"hdlr").context("no hdlr box")?;
    let mut r = Reader::new(hdlr);
    r.skip(8)?; // version, flags, pre_defined
    if r.take(4)? != b"vide" {
        return Ok(None);
    }
    let mdhd = child(&mdia, b"mdhd").context("no mdhd box")?;
    let mut r = Reader::new(mdhd);
    let version = r.u8()?;
    r.skip(3)?;
    if version == 1 {
        r.skip(16)?;
    } else {
        r.skip(8)?;
    }
    let timescale = r.u32()?;
    if timescale == 0 {
        bail!("timescale 0");
    }
    let minf = boxes(child(&mdia, b"minf").context("no minf box")?, counter)?;
    let stbl = boxes(child(&minf, b"stbl").context("no stbl box")?, counter)?;

    // Sample description: avc1/avc3 with its avcC.
    let stsd = child(&stbl, b"stsd").context("no stsd box")?;
    let mut r = Reader::new(stsd);
    r.skip(4)?;
    if r.u32()? == 0 {
        bail!("no sample description");
    }
    let entries = boxes(&stsd[8..], counter)?;
    let Some((kind, entry)) = entries.first() else {
        bail!("no sample description");
    };
    if kind != b"avc1" && kind != b"avc3" {
        return Ok(None);
    }
    // VisualSampleEntry: 6 reserved + 2 index + 16 predefined, then the size.
    let mut r = Reader::new(entry);
    r.skip(24)?;
    let width = u32::from(r.u16()?);
    let height = u32::from(r.u16()?);
    let children = boxes(entry.get(78..).context("short sample entry")?, counter)?;
    let avcc = child(&children, b"avcC").context("no avcC box")?;
    let (nal_length, params) = read_avcc(avcc)?;

    let durations = read_stts(child(&stbl, b"stts").context("no stts box")?)?;
    let sizes = read_stsz(child(&stbl, b"stsz").context("no stsz box")?)?;
    let chunks = match (child(&stbl, b"stco"), child(&stbl, b"co64")) {
        (Some(b), _) => read_offsets(b, false)?,
        (None, Some(b)) => read_offsets(b, true)?,
        _ => bail!("no chunk offsets"),
    };
    let stsc = read_stsc(child(&stbl, b"stsc").context("no stsc box")?)?;

    // Where each sample is: chunks hold runs of samples (stsc).
    let mut samples = Vec::with_capacity(sizes.len().min(MAX_SAMPLES));
    let mut next = 0usize;
    for (i, &chunk_offset) in chunks.iter().enumerate() {
        let chunk_number = i as u32 + 1;
        let per_chunk = stsc
            .iter()
            .rev()
            .find(|(first, _)| *first <= chunk_number)
            .map_or(0, |(_, n)| *n) as usize;
        let mut offset = chunk_offset;
        for _ in 0..per_chunk {
            let Some(&size) = sizes.get(next) else { break };
            if offset.checked_add(size).is_none_or(|end| end > file.len()) {
                bail!("sample past the end of the file");
            }
            let duration = durations.get(next).copied().unwrap_or(0);
            samples.push(Sample {
                offset,
                size,
                duration,
            });
            offset += size;
            next += 1;
            if samples.len() >= MAX_SAMPLES {
                break;
            }
        }
        if samples.len() >= MAX_SAMPLES || next >= sizes.len() {
            break;
        }
    }
    if samples.is_empty() {
        bail!("no samples");
    }
    Ok(Some(Clip {
        width,
        height,
        timescale,
        nal_length,
        params,
        samples,
    }))
}

fn read_avcc(data: &[u8]) -> Result<(usize, Vec<Vec<u8>>)> {
    let mut r = Reader::new(data);
    r.skip(4)?; // version, profile, compatibility, level
    let nal_length = usize::from(r.u8()? & 3) + 1;
    if nal_length == 3 {
        bail!("NAL length of 3 bytes not supported");
    }
    let mut params = Vec::new();
    let sps = r.u8()? & 0x1f;
    for _ in 0..sps {
        let n = usize::from(r.u16()?);
        params.push(r.take(n)?.to_vec());
    }
    let pps = r.u8()?;
    for _ in 0..pps {
        let n = usize::from(r.u16()?);
        params.push(r.take(n)?.to_vec());
    }
    if params.is_empty() {
        bail!("no parameter sets");
    }
    Ok((nal_length, params))
}

/// Duration of each sample (run-length coded).
fn read_stts(data: &[u8]) -> Result<Vec<u32>> {
    let mut r = Reader::new(data);
    r.skip(4)?;
    let entries = r.u32()?;
    let mut out = Vec::new();
    for _ in 0..entries {
        let count = r.u32()? as usize;
        let delta = r.u32()?;
        let room = MAX_SAMPLES.saturating_sub(out.len());
        out.extend(std::iter::repeat_n(delta, count.min(room)));
        if out.len() >= MAX_SAMPLES {
            break;
        }
    }
    Ok(out)
}

fn read_stsz(data: &[u8]) -> Result<Vec<usize>> {
    let mut r = Reader::new(data);
    r.skip(4)?;
    let fixed = r.u32()? as usize;
    let count = (r.u32()? as usize).min(MAX_SAMPLES);
    if fixed != 0 {
        return Ok(vec![fixed; count]);
    }
    (0..count).map(|_| Ok(r.u32()? as usize)).collect()
}

fn read_offsets(data: &[u8], wide: bool) -> Result<Vec<usize>> {
    let mut r = Reader::new(data);
    r.skip(4)?;
    let count = (r.u32()? as usize).min(MAX_SAMPLES);
    (0..count)
        .map(|_| {
            let v = if wide { r.u64()? } else { u64::from(r.u32()?) };
            usize::try_from(v).context("offset too large")
        })
        .collect()
}

/// (first chunk, samples per chunk) runs.
fn read_stsc(data: &[u8]) -> Result<Vec<(u32, u32)>> {
    let mut r = Reader::new(data);
    r.skip(4)?;
    let count = (r.u32()? as usize).min(MAX_SAMPLES);
    (0..count)
        .map(|_| {
            let first = r.u32()?;
            let per = r.u32()?;
            r.skip(4)?;
            Ok((first, per))
        })
        .collect()
}

/// A sample as an Annex B frame (start codes instead of lengths), with the
/// parameter sets in front of the first one.
pub fn annex_b(clip: &Clip, file: &[u8], index: usize) -> Result<Vec<u8>> {
    let s = clip.samples.get(index).context("no such sample")?;
    let end = s.offset.checked_add(s.size).context("size overflow")?;
    let data = file.get(s.offset..end).context("sample out of the file")?;
    let mut out = Vec::with_capacity(data.len() + 64);
    if index == 0 {
        for p in &clip.params {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(p);
        }
    }
    let mut r = Reader::new(data);
    while r.pos < data.len() {
        let n = match clip.nal_length {
            1 => usize::from(r.u8()?),
            2 => usize::from(r.u16()?),
            _ => r.u32()? as usize,
        };
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(r.take(n)?);
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    fn full(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = vec![0, 0, 0, 0];
        b.extend_from_slice(body);
        bx(kind, &b)
    }

    fn be32(v: u32) -> [u8; 4] {
        v.to_be_bytes()
    }

    /// A tiny MP4 with the given Annex B frames (each a list of NAL units)
    /// and SPS/PPS: what the tests and the demo can decode.
    pub(crate) fn make_mp4(
        params: &[Vec<u8>],
        frames: &[Vec<Vec<u8>>],
        w: u16,
        h: u16,
        delta: u32,
    ) -> Vec<u8> {
        let mut mdat = Vec::new();
        let mut sizes = Vec::new();
        for f in frames {
            let mut s = Vec::new();
            for nal in f {
                s.extend_from_slice(&be32(nal.len() as u32));
                s.extend_from_slice(nal);
            }
            sizes.push(s.len() as u32);
            mdat.extend_from_slice(&s);
        }
        let ftyp = bx(b"ftyp", b"isom\0\0\0\0isomavc1");
        let mut avcc = vec![1, 66, 0, 30, 0xff, 0xe0 | 1];
        avcc.extend_from_slice(&(params[0].len() as u16).to_be_bytes());
        avcc.extend_from_slice(&params[0]);
        avcc.push((params.len() - 1) as u8);
        for p in &params[1..] {
            avcc.extend_from_slice(&(p.len() as u16).to_be_bytes());
            avcc.extend_from_slice(p);
        }
        let mut entry = vec![0u8; 24];
        entry.extend_from_slice(&w.to_be_bytes());
        entry.extend_from_slice(&h.to_be_bytes());
        entry.extend_from_slice(&[0u8; 50]);
        entry.extend_from_slice(&bx(b"avcC", &avcc));
        let mut stsd_body = be32(1).to_vec();
        stsd_body.extend_from_slice(&bx(b"avc1", &entry));
        let mut stts = be32(1).to_vec();
        stts.extend_from_slice(&be32(frames.len() as u32));
        stts.extend_from_slice(&be32(delta));
        let mut stsz = be32(0).to_vec();
        stsz.extend_from_slice(&be32(frames.len() as u32));
        for s in &sizes {
            stsz.extend_from_slice(&be32(*s));
        }
        let mut stsc = be32(1).to_vec();
        stsc.extend_from_slice(&be32(1));
        stsc.extend_from_slice(&be32(frames.len() as u32));
        stsc.extend_from_slice(&be32(1));
        let build = |mdat_offset: u32| {
            let mut stco = be32(1).to_vec();
            stco.extend_from_slice(&be32(mdat_offset));
            let stbl = [
                full(b"stsd", &stsd_body),
                full(b"stts", &stts),
                full(b"stsz", &stsz),
                full(b"stsc", &stsc),
                full(b"stco", &stco),
            ]
            .concat();
            let minf = bx(b"minf", &bx(b"stbl", &stbl));
            let mut hdlr = vec![0u8; 4];
            hdlr.extend_from_slice(b"vide");
            hdlr.extend_from_slice(&[0u8; 12]);
            let mut mdhd = vec![0u8; 8];
            mdhd.extend_from_slice(&be32(1000));
            mdhd.extend_from_slice(&be32(0));
            mdhd.extend_from_slice(&[0u8; 4]);
            let mdia = bx(
                b"mdia",
                &[full(b"mdhd", &mdhd), full(b"hdlr", &hdlr), minf].concat(),
            );
            bx(b"moov", &bx(b"trak", &mdia))
        };
        let moov_len = build(0).len();
        let mdat_offset = (ftyp.len() + moov_len + 8) as u32;
        [ftyp, build(mdat_offset), bx(b"mdat", &mdat)].concat()
    }

    #[test]
    fn reads_samples_and_parameter_sets() {
        let params = vec![vec![0x67, 1, 2, 3], vec![0x68, 4, 5]];
        let frames = vec![vec![vec![0x65, 9, 9, 9]], vec![vec![0x41, 7], vec![0x41, 8, 8]]];
        let file = make_mp4(&params, &frames, 320, 180, 40);
        let clip = parse(&file).unwrap();
        assert_eq!((clip.width, clip.height, clip.timescale), (320, 180, 1000));
        assert_eq!(clip.params, params);
        assert_eq!(clip.samples.len(), 2);
        assert_eq!(clip.samples[1].duration, 40);
        assert_eq!(
            annex_b(&clip, &file, 0).unwrap(),
            [
                &[0, 0, 0, 1, 0x67, 1, 2, 3, 0, 0, 0, 1, 0x68, 4, 5][..],
                &[0, 0, 0, 1, 0x65, 9, 9, 9]
            ]
            .concat()
        );
        assert_eq!(
            annex_b(&clip, &file, 1).unwrap(),
            vec![0, 0, 0, 1, 0x41, 7, 0, 0, 0, 1, 0x41, 8, 8]
        );
    }

    #[test]
    fn broken_files_are_errors_not_panics() {
        let params = vec![vec![0x67, 1], vec![0x68, 2]];
        let file = make_mp4(&params, &[vec![vec![0x65, 1, 2]]], 16, 16, 40);
        assert!(parse(&file).is_ok());
        // Every truncation and a few corrupted bytes: never a panic.
        for cut in 0..file.len() {
            let _ = parse(&file[..cut]);
        }
        for i in 0..file.len() {
            let mut bad = file.clone();
            bad[i] ^= 0xff;
            if let Ok(clip) = parse(&bad) {
                for k in 0..clip.samples.len() {
                    let _ = annex_b(&clip, &bad, k);
                }
            }
        }
        assert!(parse(b"not an mp4 at all").is_err());
        // A box claiming a huge size.
        let mut huge = b"\0\0\0\x10ftypisom\0\0\0\0".to_vec();
        huge.extend_from_slice(b"\0\0\0\x01moov\xff\xff\xff\xff\xff\xff\xff\xff");
        assert!(parse(&huge).is_err());
    }
}
