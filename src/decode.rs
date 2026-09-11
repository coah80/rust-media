use crate::Pixels;
use mp4::ReadBox;
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
            let (packet, idr) = annex_b(&sample.bytes, self.length)?;
            if idr {
                self.decoder = Decoder::new();
                self.decoder
                    .decode(&self.header)
                    .map_err(|_| "Invalid video configuration")?;
            }
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

struct FragmentSegment {
    range: Range<u64>,
    start: u64,
    duration: u64,
}

struct FragmentPlan {
    segments: Vec<FragmentSegment>,
    timescale: u32,
}

struct FragmentConfig {
    decoder: Decoder,
    header: Vec<u8>,
    track: u32,
    length: usize,
    scale: f64,
    default_duration: Option<u32>,
    default_size: Option<u32>,
    default_flags: Option<u32>,
}

type SegmentBoxes = (Vec<(mp4::MoofBox, u64)>, Vec<Range<u64>>);

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
    track: u32,
    default_duration: Option<u32>,
    default_size: Option<u32>,
    default_flags: Option<u32>,
    segments: Vec<FragmentSegment>,
    segment: usize,
    segment_scale: f64,
}

impl<R: Read + Seek> FragmentVideo<R> {
    fn new(mut reader: R, header_reader: R, size: u64) -> Result<Self, String> {
        if let Some(plan) = fragment_plan(&mut reader, size)? {
            return Self::new_indexed(reader, header_reader, plan);
        }
        Self::new_eager(reader, header_reader, size)
    }

    fn new_indexed(reader: R, mut header_reader: R, plan: FragmentPlan) -> Result<Self, String> {
        let header_end = plan
            .segments
            .first()
            .ok_or("The video contains no media segments")?
            .range
            .start;
        header_reader
            .seek(SeekFrom::Start(0))
            .map_err(|_| "Could not seek video initialization")?;
        let parsed = mp4::Mp4Reader::read_header(header_reader, header_end)
            .map_err(|_| "This video is not a supported fragmented MP4")?;
        let config = fragment_config(&parsed)?;
        let duration = plan
            .segments
            .last()
            .and_then(|segment| segment.start.checked_add(segment.duration))
            .ok_or("Invalid video segment duration")? as f64
            / f64::from(plan.timescale);
        let mut video = Self {
            reader,
            decoder: config.decoder,
            header: config.header,
            samples: Vec::new(),
            length: config.length,
            scale: config.scale,
            next: 0,
            pending: Vec::new(),
            duration,
            track: config.track,
            default_duration: config.default_duration,
            default_size: config.default_size,
            default_flags: config.default_flags,
            segments: plan.segments,
            segment: 0,
            segment_scale: f64::from(plan.timescale),
        };
        video.load_segment(0)?;
        Ok(video)
    }

    fn new_eager(mut reader: R, header_reader: R, size: u64) -> Result<Self, String> {
        let (moof_offsets, mdats) = top_level_boxes(&mut reader, size)?;
        let parsed = mp4::Mp4Reader::read_header(header_reader, size)
            .map_err(|_| "This video is not a supported fragmented MP4")?;
        if parsed.moofs.len() != moof_offsets.len() {
            return Err("The video fragment table is inconsistent".into());
        }
        let config = fragment_config(&parsed)?;
        let mut samples = Vec::new();
        let mut expected_time = 0u64;
        for (moof, moof_offset) in parsed.moofs.iter().zip(moof_offsets) {
            let traf = moof
                .trafs
                .iter()
                .find(|traf| traf.tfhd.track_id == config.track)
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
                    .or(config.default_duration)
                    .ok_or("Missing video sample duration")?;
                let sample_size = run
                    .sample_sizes
                    .get(index)
                    .copied()
                    .or(traf.tfhd.default_sample_size)
                    .or(config.default_size)
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
                    .or(config.default_flags)
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
            decoder: config.decoder,
            header: config.header,
            samples,
            length: config.length,
            scale: config.scale,
            next: 0,
            pending: Vec::new(),
            duration: expected_time as f64 / config.scale,
            track: config.track,
            default_duration: config.default_duration,
            default_size: config.default_size,
            default_flags: config.default_flags,
            segments: Vec::new(),
            segment: 0,
            segment_scale: config.scale,
        })
    }

    fn load_segment(&mut self, index: usize) -> Result<(), String> {
        let segment = self.segments.get(index).ok_or("Missing video segment")?;
        let (moofs, mdats) = segment_boxes(&mut self.reader, segment.range.clone())?;
        let mut samples = Vec::new();
        let mut expected_time = None;
        for (moof, moof_offset) in moofs {
            let traf = moof
                .trafs
                .iter()
                .find(|traf| traf.tfhd.track_id == self.track)
                .ok_or("Missing video fragment track")?;
            let run = traf.trun.as_ref().ok_or("Missing video fragment run")?;
            if run.sample_count > 100_000 {
                return Err("Video fragment exceeds the sample limit".into());
            }
            let mut time = traf
                .tfdt
                .as_ref()
                .map(|value| value.base_media_decode_time)
                .or(expected_time)
                .ok_or("Missing video fragment timestamp")?;
            if expected_time.is_some_and(|expected| expected != time) {
                return Err("Video fragments are not contiguous".into());
            }
            let base = traf.tfhd.base_data_offset.unwrap_or(moof_offset);
            let mut position = base
                .checked_add_signed(i64::from(
                    run.data_offset.ok_or("Missing video fragment offset")?,
                ))
                .ok_or("Invalid video fragment offset")?;
            for sample_index in 0..run.sample_count as usize {
                let duration = run
                    .sample_durations
                    .get(sample_index)
                    .copied()
                    .or(traf.tfhd.default_sample_duration)
                    .or(self.default_duration)
                    .ok_or("Missing video sample duration")?;
                let sample_size = run
                    .sample_sizes
                    .get(sample_index)
                    .copied()
                    .or(traf.tfhd.default_sample_size)
                    .or(self.default_size)
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
                    .get(sample_index)
                    .copied()
                    .or(if sample_index == 0 {
                        run.first_sample_flags
                    } else {
                        None
                    })
                    .or(traf.tfhd.default_sample_flags)
                    .or(self.default_flags)
                    .unwrap_or(0);
                let cts = run.sample_cts.get(sample_index).copied().unwrap_or(0);
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
            expected_time = Some(time);
        }
        if samples.is_empty() {
            return Err("The video segment contains no samples".into());
        }
        self.samples = samples;
        self.next = 0;
        self.segment = index;
        Ok(())
    }

    fn seek(&mut self, seconds: f64) -> Result<(), String> {
        if !self.segments.is_empty() {
            let target = seconds.max(0.) * self.segment_scale;
            let segment = self
                .segments
                .iter()
                .enumerate()
                .filter(|(_, segment)| segment.start as f64 <= target)
                .map(|(index, _)| index)
                .next_back()
                .unwrap_or(0)
                .saturating_sub(1);
            self.load_segment(segment)?;
        }
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
        while self.pending.len() < 16 {
            if self.next >= self.samples.len() {
                if self.segments.is_empty() || self.segment + 1 >= self.segments.len() {
                    break;
                }
                self.load_segment(self.segment + 1)?;
            }
            let sample = &self.samples[self.next];
            let mut bytes = vec![0; sample.size];
            self.reader
                .seek(SeekFrom::Start(sample.offset))
                .and_then(|_| self.reader.read_exact(&mut bytes))
                .map_err(|_| "Could not read video")?;
            self.next += 1;
            let time = (sample.start as f64 + f64::from(sample.rendering_offset)) / self.scale;
            let (packet, idr) = annex_b(&bytes, self.length)?;
            if idr {
                self.decoder = Decoder::new();
                self.decoder
                    .decode(&self.header)
                    .map_err(|_| "Invalid video configuration")?;
            }
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

fn fragment_config<R: Read + Seek>(parsed: &mp4::Mp4Reader<R>) -> Result<FragmentConfig, String> {
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
    Ok(FragmentConfig {
        decoder,
        header,
        track: track_id,
        length: usize::from(avc.avcc.length_size_minus_one & 3) + 1,
        scale: f64::from(track.timescale()),
        default_duration: trex.map(|value| value.default_sample_duration),
        default_size: trex.map(|value| value.default_sample_size),
        default_flags: trex.map(|value| value.default_sample_flags),
    })
}

fn fragment_plan<R: Read + Seek>(
    reader: &mut R,
    size: u64,
) -> Result<Option<FragmentPlan>, String> {
    let mut offset = 0u64;
    while offset < size {
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|_| "Could not seek video initialization")?;
        let (kind, payload, end) = box_header(reader, offset, size)?;
        if &kind == b"moof" {
            return Ok(None);
        }
        if &kind != b"sidx" {
            offset = end;
            continue;
        }
        let mut version_flags = [0; 4];
        reader
            .read_exact(&mut version_flags)
            .map_err(|_| "Could not read video segment index")?;
        let version = version_flags[0];
        let _reference_id = read_u32(reader)?;
        let timescale = read_u32(reader)?;
        if timescale == 0 {
            return Err("Invalid video segment timescale".into());
        }
        let (earliest, first_offset) = match version {
            0 => (u64::from(read_u32(reader)?), u64::from(read_u32(reader)?)),
            1 => (read_u64(reader)?, read_u64(reader)?),
            _ => return Err("Unsupported video segment index".into()),
        };
        let mut reserved_and_count = [0; 4];
        reader
            .read_exact(&mut reserved_and_count)
            .map_err(|_| "Could not read video segment index")?;
        let count = u16::from_be_bytes(reserved_and_count[2..].try_into().unwrap()) as usize;
        let expected = 4u64
            .checked_add(4)
            .and_then(|value| value.checked_add(4))
            .and_then(|value| value.checked_add(if version == 0 { 8 } else { 16 }))
            .and_then(|value| value.checked_add(4))
            .and_then(|value| value.checked_add((count as u64).saturating_mul(12)))
            .ok_or("Invalid video segment index")?;
        if count == 0 || count > 10_000 || expected > payload {
            return Err("Invalid video segment index".into());
        }
        let mut position = end
            .checked_add(first_offset)
            .ok_or("Invalid video segment offset")?;
        let mut time = earliest;
        let mut segments = Vec::with_capacity(count);
        for _ in 0..count {
            let reference = read_u32(reader)?;
            let duration = read_u32(reader)?;
            let _sap = read_u32(reader)?;
            let length = u64::from(reference & 0x7fff_ffff);
            let segment_end = position
                .checked_add(length)
                .filter(|end| *end <= size)
                .ok_or("Invalid video segment offset")?;
            if reference & 0x8000_0000 != 0 || length < 16 || duration == 0 {
                return Err("Unsupported video segment index".into());
            }
            segments.push(FragmentSegment {
                range: position..segment_end,
                start: time,
                duration: u64::from(duration),
            });
            position = segment_end;
            time = time
                .checked_add(u64::from(duration))
                .ok_or("Invalid video segment duration")?;
        }
        return Ok(Some(FragmentPlan {
            segments,
            timescale,
        }));
    }
    Ok(None)
}

fn segment_boxes<R: Read + Seek>(
    reader: &mut R,
    range: Range<u64>,
) -> Result<SegmentBoxes, String> {
    let mut offset = range.start;
    let mut moofs = Vec::new();
    let mut mdats = Vec::new();
    while offset < range.end {
        reader
            .seek(SeekFrom::Start(offset))
            .map_err(|_| "Could not seek video segment")?;
        let (kind, payload, end) = box_header(reader, offset, range.end)?;
        match &kind {
            b"moof" => {
                if reader.stream_position().ok() != Some(offset + 8) {
                    return Err("Unsupported extended video fragment".into());
                }
                let moof = mp4::MoofBox::read_box(reader, payload + 8)
                    .map_err(|_| "Could not parse video fragment")?;
                moofs.push((moof, offset));
            }
            b"mdat" => mdats.push(end - payload..end),
            _ => {}
        }
        offset = end;
    }
    if moofs.is_empty() || mdats.is_empty() {
        return Err("Video segment is missing media fragments".into());
    }
    Ok((moofs, mdats))
}

fn box_header<R: Read>(
    reader: &mut R,
    offset: u64,
    limit: u64,
) -> Result<([u8; 4], u64, u64), String> {
    let mut header = [0; 8];
    reader
        .read_exact(&mut header)
        .map_err(|_| "Could not read video box")?;
    let short_size = u32::from_be_bytes(header[..4].try_into().unwrap());
    let mut header_size = 8u64;
    let size = if short_size == 1 {
        header_size = 16;
        read_u64(reader)?
    } else if short_size == 0 {
        limit - offset
    } else {
        u64::from(short_size)
    };
    let end = offset
        .checked_add(size)
        .filter(|end| size >= header_size && *end <= limit)
        .ok_or("Invalid video box")?;
    Ok((header[4..8].try_into().unwrap(), size - header_size, end))
}

fn read_u32<R: Read>(reader: &mut R) -> Result<u32, String> {
    let mut bytes = [0; 4];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| "Could not read video segment index")?;
    Ok(u32::from_be_bytes(bytes))
}

fn read_u64<R: Read>(reader: &mut R) -> Result<u64, String> {
    let mut bytes = [0; 8];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| "Could not read video segment index")?;
    Ok(u64::from_be_bytes(bytes))
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

fn annex_b(bytes: &[u8], length: usize) -> Result<(Vec<u8>, bool), String> {
    let mut output = Vec::with_capacity(bytes.len() + 32);
    let mut cursor = 0usize;
    let mut idr = false;
    while cursor < bytes.len() {
        let size = bytes
            .get(cursor..cursor + length)
            .ok_or("Truncated video packet")?
            .iter()
            .fold(0usize, |size, byte| (size << 8) | usize::from(*byte));
        cursor += length;
        let end = cursor.checked_add(size).ok_or("Invalid video packet")?;
        let nal = bytes.get(cursor..end).ok_or("Truncated video packet")?;
        idr |= nal.first().is_some_and(|byte| byte & 0x1f == 5);
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
        cursor = end;
    }
    Ok((output, idr))
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
