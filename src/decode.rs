use crate::Pixels;
use rusty_h264_decoder::Decoder;
use std::{
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

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
            let time = (sample.start_time as f64 + f64::from(sample.rendering_offset)) / self.scale
                - self.offset;
            let packet = annex_b(&sample.bytes, self.length)?;
            if let Some(frame) = self
                .decoder
                .decode(&packet)
                .map_err(|_| "Could not decode this video")?
            {
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

struct FragmentSample {
    offset: u64,
    size: usize,
    start: u64,
    rendering_offset: i32,
    sync: bool,
}

pub struct FragmentVideo<R> {
    reader: R,
    decoder: Decoder,
    header: Vec<u8>,
    samples: Vec<FragmentSample>,
    length: usize,
    scale: f64,
    next: usize,
    pending: Vec<(f64, Pixels)>,
    duration: f64,
}

impl<R: Read + Seek> FragmentVideo<R> {
    fn new(mut reader: R, header_reader: R, size: u64) -> Result<Self, String> {
        let (moof_offsets, mdats) = top_level_boxes(&mut reader, size)?;
        let parsed = mp4::Mp4Reader::read_header(header_reader, size)
            .map_err(|_| "This video is not a supported fragmented MP4")?;
        if parsed.moofs.len() != moof_offsets.len() {
            return Err("The video fragment table is inconsistent".into());
        }
        let track = parsed
            .tracks()
            .values()
            .find(|track| track.media_type().ok() == Some(mp4::MediaType::H264))
            .ok_or("This video codec is not supported yet")?;
        if track.width() == 0
            || track.height() == 0
            || track.width() > 1920
            || track.height() > 1920
            || track.timescale() == 0
        {
            return Err("This video exceeds the playback limit".into());
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
        let trex = parsed
            .moov
            .mvex
            .as_ref()
            .map(|mvex| &mvex.trex)
            .filter(|trex| trex.track_id == track_id);
        let mut samples = Vec::new();
        let mut expected_time = 0u64;
        for (moof, moof_offset) in parsed.moofs.iter().zip(moof_offsets) {
            let traf = moof
                .trafs
                .iter()
                .find(|traf| traf.tfhd.track_id == track_id)
                .ok_or("Missing video fragment track")?;
            let run = traf.trun.as_ref().ok_or("Missing video fragment run")?;
            if run.sample_count > 100_000 {
                return Err("Video fragment exceeds the sample limit".into());
            }
            let mut time = traf
                .tfdt
                .as_ref()
                .map_or(expected_time, |value| value.base_media_decode_time);
            if time != expected_time {
                return Err("Video fragments are not contiguous".into());
            }
            let base = traf.tfhd.base_data_offset.unwrap_or(moof_offset);
            let mut position = base
                .checked_add_signed(i64::from(
                    run.data_offset.ok_or("Missing video fragment offset")?,
                ))
                .ok_or("Invalid video fragment offset")?;
            for index in 0..run.sample_count as usize {
                if samples.len() >= 1_000_000 {
                    return Err("Video exceeds the sample limit".into());
                }
                let duration = run
                    .sample_durations
                    .get(index)
                    .copied()
                    .or(traf.tfhd.default_sample_duration)
                    .or_else(|| trex.map(|value| value.default_sample_duration))
                    .ok_or("Missing video sample duration")?;
                let sample_size = run
                    .sample_sizes
                    .get(index)
                    .copied()
                    .or(traf.tfhd.default_sample_size)
                    .or_else(|| trex.map(|value| value.default_sample_size))
                    .ok_or("Missing video sample size")? as usize;
                let end = position
                    .checked_add(sample_size as u64)
                    .ok_or("Invalid video sample offset")?;
                if duration == 0
                    || sample_size == 0
                    || sample_size > 8 * 1024 * 1024
                    || !mdats
                        .iter()
                        .any(|range| range.start <= position && end <= range.end)
                {
                    return Err("Video sample is outside its media fragment".into());
                }
                let flags = run
                    .sample_flags
                    .get(index)
                    .copied()
                    .or(if index == 0 {
                        run.first_sample_flags
                    } else {
                        None
                    })
                    .or(traf.tfhd.default_sample_flags)
                    .or_else(|| trex.map(|value| value.default_sample_flags))
                    .unwrap_or(0);
                let cts = run.sample_cts.get(index).copied().unwrap_or(0);
                let rendering_offset = if run.version == 0 {
                    i32::try_from(cts).map_err(|_| "Invalid video composition time")?
                } else {
                    cts as i32
                };
                samples.push(FragmentSample {
                    offset: position,
                    size: sample_size,
                    start: time,
                    rendering_offset,
                    sync: flags & 0x10000 == 0,
                });
                position = end;
                time = time
                    .checked_add(u64::from(duration))
                    .ok_or("Video timestamp overflow")?;
            }
            expected_time = time;
        }
        if samples.is_empty() {
            return Err("The video contains no samples".into());
        }
        Ok(Self {
            reader,
            decoder,
            header,
            samples,
            length: usize::from(avc.avcc.length_size_minus_one & 3) + 1,
            scale: f64::from(track.timescale()),
            next: 0,
            pending: Vec::new(),
            duration: expected_time as f64 / f64::from(track.timescale()),
        })
    }

    fn seek(&mut self, seconds: f64) -> Result<(), String> {
        let target = seconds.max(0.) * self.scale;
        let next = self
            .samples
            .iter()
            .enumerate()
            .filter(|(_, sample)| {
                sample.sync && sample.start as f64 + f64::from(sample.rendering_offset) <= target
            })
            .map(|(index, _)| index)
            .next_back()
            .unwrap_or(0);
        self.decoder = Decoder::new();
        self.decoder
            .decode(&self.header)
            .map_err(|_| "Invalid video configuration")?;
        self.next = next;
        self.pending.clear();
        Ok(())
    }

    fn frame(&mut self) -> Result<Option<(f64, Pixels)>, String> {
        while self.pending.len() < 16 && self.next < self.samples.len() {
            let sample = &self.samples[self.next];
            let mut bytes = vec![0; sample.size];
            self.reader
                .seek(SeekFrom::Start(sample.offset))
                .and_then(|_| self.reader.read_exact(&mut bytes))
                .map_err(|_| "Could not read video")?;
            self.next += 1;
            let time = (sample.start as f64 + f64::from(sample.rendering_offset)) / self.scale;
            let packet = annex_b(&bytes, self.length)?;
            if let Some(frame) = self
                .decoder
                .decode(&packet)
                .map_err(|_| "Could not decode this video")?
            {
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

pub enum MediaVideo<R> {
    Standard(Box<Video<R>>),
    Fragmented(Box<FragmentVideo<R>>),
}

impl<R: Read + Seek> MediaVideo<R> {
    pub fn standard(reader: R, size: u64) -> Result<Self, String> {
        Video::new(reader, size).map(Box::new).map(Self::Standard)
    }

    pub fn fragmented(reader: R, header_reader: R, size: u64) -> Result<Self, String> {
        FragmentVideo::new(reader, header_reader, size)
            .map(Box::new)
            .map(Self::Fragmented)
    }

    pub fn duration(&self) -> f64 {
        match self {
            Self::Standard(video) => video.duration,
            Self::Fragmented(video) => video.duration,
        }
    }

    pub fn has_audio(&self) -> bool {
        match self {
            Self::Standard(video) => video.audio,
            Self::Fragmented(_) => false,
        }
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        match self {
            Self::Standard(video) => video.seek(seconds),
            Self::Fragmented(video) => video.seek(seconds),
        }
    }

    pub fn frame(&mut self) -> Result<Option<(f64, Pixels)>, String> {
        match self {
            Self::Standard(video) => video.frame(),
            Self::Fragmented(video) => video.frame(),
        }
    }
}

fn top_level_boxes<R: Read + Seek>(
    reader: &mut R,
    size: u64,
) -> Result<(Vec<u64>, Vec<Range<u64>>), String> {
    let mut offset = 0u64;
    let mut moofs = Vec::new();
    let mut mdats = Vec::new();
    while offset < size {
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|_| "Could not seek video fragments")?;
        let mut header = [0; 8];
        reader
            .read_exact(&mut header)
            .map_err(|_| "Could not read video fragments")?;
        let short_size = u32::from_be_bytes(header[..4].try_into().unwrap());
        let mut header_size = 8u64;
        let box_size = if short_size == 1 {
            let mut extended = [0; 8];
            reader
                .read_exact(&mut extended)
                .map_err(|_| "Could not read video fragments")?;
            header_size = 16;
            u64::from_be_bytes(extended)
        } else if short_size == 0 {
            size - offset
        } else {
            u64::from(short_size)
        };
        let end = offset
            .checked_add(box_size)
            .filter(|end| box_size >= header_size && *end <= size)
            .ok_or("Invalid video fragment box")?;
        match &header[4..8] {
            b"moof" => moofs.push(offset),
            b"mdat" => mdats.push(offset + header_size..end),
            _ => {}
        }
        offset = end;
    }
    Ok((moofs, mdats))
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
