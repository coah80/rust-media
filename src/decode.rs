use crate::Pixels;
use rusty_h264_decoder::Decoder;
use std::io::{Read, Seek};

pub struct Video<R> {
    reader: mp4::Mp4Reader<R>,
    decoder: Decoder,
    header: Vec<u8>,
    track: u32,
    length: usize,
    scale: f64,
    offset: f64,
    next: u32,
    count: u32,
    pending: Vec<(f64, Pixels)>,
    pub duration: f64,
    pub audio: bool,
}

impl<R: Read + Seek> Video<R> {
    pub fn new(reader: R, size: u64) -> Result<Self, String> {
        let reader = mp4::Mp4Reader::read_header(reader, size)
            .map_err(|_| "This video is not a supported MP4")?;
        let track = reader
            .tracks()
            .values()
            .find(|track| track.media_type().ok() == Some(mp4::MediaType::H264))
            .ok_or("This video codec is not supported yet")?;
        if track.width() == 0
            || track.height() == 0
            || track.width() > 1920
            || track.height() > 1920
            || track.sample_count() == 0
            || track.sample_count() > 1_000_000
        {
            return Err("This video exceeds the playback limit".into());
        }
        for track in reader.tracks().values() {
            let sizes = &track.trak.mdia.minf.stbl.stsz;
            if sizes.sample_size > 8 * 1024 * 1024
                || sizes
                    .sample_sizes
                    .iter()
                    .any(|size| *size > 8 * 1024 * 1024)
            {
                return Err("Video sample exceeds the size limit".into());
            }
        }
        let avc = track
            .trak
            .mdia
            .minf
            .stbl
            .stsd
            .avc1
            .as_ref()
            .ok_or("Missing video configuration")?;
        let mut header = Vec::new();
        for parameter in [
            &avc.avcc.sequence_parameter_sets,
            &avc.avcc.picture_parameter_sets,
        ] {
            for nal in parameter {
                header.extend_from_slice(&[0, 0, 0, 1]);
                header.extend_from_slice(&nal.bytes);
            }
        }
        let mut decoder = Decoder::new();
        decoder
            .decode(&header)
            .map_err(|_| "Invalid video configuration")?;
        let track_id = track.track_id();
        let length = usize::from(avc.avcc.length_size_minus_one & 3) + 1;
        let scale = f64::from(track.timescale().max(1));
        let count = track.sample_count();
        let mut offset = 0.;
        if let Some(edits) = track
            .trak
            .edts
            .as_ref()
            .and_then(|edits| edits.elst.as_ref())
        {
            if edits.entries.len() > 1 {
                return Err("Multiple video edit segments are not supported yet".into());
            }
            if let Some(edit) = edits.entries.first() {
                if edit.media_rate != 1
                    || edit.media_rate_fraction != 0
                    || edit.media_time > i64::MAX as u64
                {
                    return Err("This video edit is not supported yet".into());
                }
                offset = edit.media_time as f64 / scale;
            }
        }
        let duration = track.trak.tkhd.duration as f64 / f64::from(reader.timescale().max(1));
        let audio = reader
            .tracks()
            .values()
            .any(|track| track.track_type().ok() == Some(mp4::TrackType::Audio));
        Ok(Self {
            reader,
            decoder,
            header,
            track: track_id,
            length,
            scale,
            offset,
            next: 1,
            count,
            pending: Vec::new(),
            duration,
            audio,
        })
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        let seconds = seconds + self.offset;
        let mut low = 1;
        let mut high = self.count;
        while low < high {
            let mid = (low + high).div_ceil(2);
            let sample = self
                .reader
                .read_sample(self.track, mid)
                .map_err(|_| "Could not seek video")?
                .ok_or("Missing video sample")?;
            if sample.start_time as f64 / self.scale <= seconds {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        while low > 1 {
            let sample = self
                .reader
                .read_sample(self.track, low)
                .map_err(|_| "Could not seek video")?
                .ok_or("Missing video sample")?;
            if sample.is_sync {
                break;
            }
            low -= 1;
        }
        self.decoder = Decoder::new();
        self.decoder
            .decode(&self.header)
            .map_err(|_| "Invalid video configuration")?;
        self.next = low;
        self.pending.clear();
        Ok(())
    }

    pub fn frame(&mut self) -> Result<Option<(f64, Pixels)>, String> {
        while self.pending.len() < 16 && self.next <= self.count {
            let sample = self
                .reader
                .read_sample(self.track, self.next)
                .map_err(|_| "Could not read video")?
                .ok_or("Missing video sample")?;
            self.next += 1;
            let packet = annex_b(&sample.bytes, self.length)?;
            if let Some(frame) = self
                .decoder
                .decode(&packet)
                .map_err(|_| "Could not decode this video")?
            {
                let time = (sample.start_time as f64 + f64::from(sample.rendering_offset))
                    / self.scale
                    - self.offset;
                self.pending.push((time.max(0.), pixels(frame)?));
            }
        }
        self.pending
            .sort_by(|left, right| left.0.total_cmp(&right.0));
        Ok(if self.pending.is_empty() {
            None
        } else {
            Some(self.pending.remove(0))
        })
    }
}

fn annex_b(bytes: &[u8], length: usize) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(bytes.len() + 32);
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let size = bytes
            .get(cursor..cursor + length)
            .ok_or("Truncated video packet")?
            .iter()
            .fold(0usize, |size, byte| (size << 8) | usize::from(*byte));
        cursor += length;
        let end = cursor.checked_add(size).ok_or("Invalid video packet")?;
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(bytes.get(cursor..end).ok_or("Truncated video packet")?);
        cursor = end;
    }
    Ok(output)
}

fn pixels(frame: rusty_h264_common::types::YuvFrame) -> Result<Pixels, String> {
    let w = frame.width;
    let h = frame.height;
    if w == 0
        || h == 0
        || w > 1920
        || h > 1920
        || frame.y.len() < w * h
        || frame.u.len() < w.div_ceil(2) * h.div_ceil(2)
        || frame.v.len() < frame.u.len()
    {
        return Err("Invalid video dimensions".into());
    }
    let scale = (w as f64 / 1280.).max(h as f64 / 720.).max(1.);
    let width = (w as f64 / scale) as u32;
    let height = (h as f64 / scale) as u32;
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height as usize {
        let source_y = y * h / height as usize;
        for x in 0..width as usize {
            let source_x = x * w / width as usize;
            let yy = i32::from(frame.y[source_y * w + source_x]) - 16;
            let c = source_y / 2 * w.div_ceil(2) + source_x / 2;
            let u = i32::from(frame.u[c]) - 128;
            let v = i32::from(frame.v[c]) - 128;
            for value in [
                298 * yy + 409 * v,
                298 * yy - 100 * u - 208 * v,
                298 * yy + 516 * u,
            ] {
                rgba.push(((value + 128) >> 8).clamp(0, 255) as u8);
            }
            rgba.push(255);
        }
    }
    Ok(Pixels {
        rgba,
        width,
        height,
    })
}
