use crate::Pixels;
use rff_codec::{CodecParams, CodecRegistry, Decoder};
use rff_core::{CodecId, Error, Frame, MediaType, Packet, PixelFormat, VideoFrame};
use std::io::Read;

const MAX_FILE: u64 = 32 * 1024 * 1024;

fn error(e: impl std::fmt::Display) -> String {
    format!("Media decoding failed: {e}")
}

fn guarded<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .map_err(|_| "The media decoder rejected this file".to_string())?
}

fn registry() -> CodecRegistry {
    let mut registry = CodecRegistry::new();
    rff_codec_vp9::register(&mut registry);
    rff_codec_vorbis::register(&mut registry);
    registry
}

fn decoder(params: &CodecParams) -> Result<Box<dyn Decoder>, String> {
    let mut decoder: Box<dyn Decoder> = if params.codec_id == CodecId::Avif {
        Box::new(Av1::new()?)
    } else if params.codec_id == CodecId::Hevc {
        Box::new(Hevc(rusty_h265::Decoder::new()))
    } else {
        registry().find_decoder(params.codec_id).map_err(error)?
    };
    decoder.configure(params).map_err(error)?;
    Ok(decoder)
}

struct Hevc(rusty_h265::Decoder);
impl Decoder for Hevc {
    fn send_packet(&mut self, packet: &Packet) -> rff_core::Result<()> {
        self.0
            .push_annexb(&packet.data, packet.pts)
            .map_err(|e| Error::InvalidData(e.to_string()))
    }
    fn receive_frame(&mut self) -> rff_core::Result<Frame> {
        let frame = self.0.next_frame().map_err(|e| match e {
            rusty_h265::Error::Again => Error::Again,
            rusty_h265::Error::Eof => Error::Eof,
            e => Error::InvalidData(e.to_string()),
        })?;
        let picture = &frame.picture;
        let (x, y, w, h) = picture.crop;
        if picture.chroma_format_idc != 1 {
            return Err(Error::Unsupported("HEVC requires 4:2:0 chroma".into()));
        }
        dimensions(w as u32, h as u32).map_err(Error::InvalidData)?;
        let wide = picture.bit_depth_luma == 10;
        let mut planes = Vec::new();
        let mut strides = Vec::new();
        for (i, plane) in picture.planes.iter().enumerate() {
            let divisor = if i == 0 { 1 } else { 2 };
            let (px, py, pw, ph) = (
                x / divisor,
                y / divisor,
                w.div_ceil(divisor),
                h.div_ceil(divisor),
            );
            let mut bytes = Vec::with_capacity(pw * ph * if wide { 2 } else { 1 });
            for row in py..py + ph {
                let values = plane
                    .data
                    .get(row * plane.stride + px..row * plane.stride + px + pw)
                    .ok_or_else(|| Error::InvalidData("Truncated HEVC plane".into()))?;
                for &value in values {
                    if wide {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    } else {
                        bytes.push(value as u8);
                    }
                }
            }
            planes.push(bytes);
            strides.push(pw * if wide { 2 } else { 1 });
        }
        Ok(Frame::Video(VideoFrame {
            width: w as u32,
            height: h as u32,
            format: if wide {
                PixelFormat::Yuv420p10
            } else {
                PixelFormat::Yuv420p
            },
            planes,
            strides,
            pts: frame.pts,
        }))
    }
    fn flush(&mut self) {
        self.0.flush();
    }
}

enum AudioDecoder {
    Opus(Box<rusty_opus::OpusDecoder>),
    Vorbis(Box<dyn Decoder>),
}

pub(crate) struct Audio {
    params: CodecParams,
    packets: Vec<Packet>,
    decoder: AudioDecoder,
    next: usize,
    buffer: Vec<f32>,
    position: usize,
    target: f64,
    duration: f64,
    gain: f32,
    pub error: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

impl Audio {
    fn decoder(params: &CodecParams) -> Result<AudioDecoder, String> {
        match params.codec_id {
            CodecId::Opus => {
                let h = &params.extradata;
                if h.len() < 19
                    || &h[..8] != b"OpusHead"
                    || h[8] > 15
                    || h[18] != 0
                    || !(1..=2).contains(&h[9])
                    || u16::from(h[9]) != params.channels
                {
                    return Err("Unsupported Opus channel mapping".into());
                }
                rusty_opus::OpusDecoder::new(48000, usize::from(params.channels))
                    .map(Box::new)
                    .map(AudioDecoder::Opus)
                    .map_err(error)
            }
            CodecId::Vorbis => decoder(params).map(AudioDecoder::Vorbis),
            _ => Err("This audio codec is not supported yet".into()),
        }
    }
    pub fn new(source: crate::player::MediaReader) -> Result<Self, String> {
        let size = source.size();
        let (mut params, packets, duration, _) =
            guarded(|| packets(source, size, MediaType::Audio))?;
        if params.channels == 0 || params.sample_rate == 0 {
            return Err("Missing audio format".into());
        }
        if params.codec_id == CodecId::Opus {
            params.sample_rate = 48000;
        }
        let decoder = guarded(|| Self::decoder(&params))?;
        let gain = if params.codec_id == CodecId::Opus {
            10_f32.powf(
                f32::from(i16::from_le_bytes([
                    params.extradata[16],
                    params.extradata[17],
                ])) / 5120.,
            )
        } else {
            1.
        };
        let mut audio = Self {
            params,
            packets,
            decoder,
            next: 0,
            buffer: Vec::new(),
            position: 0,
            target: 0.,
            duration,
            gain,
            error: Default::default(),
        };
        guarded(|| audio.fill())?;
        if audio.buffer.is_empty() {
            return Err("The video contains no audio samples".into());
        }
        Ok(audio)
    }
    fn fill(&mut self) -> Result<(), String> {
        self.buffer.clear();
        self.position = 0;
        while let Some(packet) = self.packets.get(self.next) {
            self.buffer.clear();
            self.next += 1;
            let pts = packet.pts.unwrap_or(0) as f64 / 1_000_000.;
            match &mut self.decoder {
                AudioDecoder::Opus(decoder) => {
                    let channels = usize::from(self.params.channels);
                    let size = opus_samples(&packet.data)?;
                    self.buffer.resize(size * channels, 0.);
                    let frames = decoder
                        .decode(&packet.data, size, &mut self.buffer)
                        .map_err(error)?;
                    self.buffer.truncate(frames * channels);
                }
                AudioDecoder::Vorbis(decoder) => {
                    decoder.send_packet(packet).map_err(error)?;
                    match decoder.receive_frame() {
                        Ok(Frame::Audio(frame)) => {
                            if frame.channels != self.params.channels
                                || frame.sample_rate != self.params.sample_rate
                                || frame.samples > 65536
                                || frame.format != rff_core::SampleFormat::S16
                            {
                                return Err("Decoded audio format exceeds limits".into());
                            }
                            let plane = frame.planes.first().ok_or("Missing audio samples")?;
                            if plane.len() != frame.samples * usize::from(frame.channels) * 2 {
                                return Err("Truncated audio samples".into());
                            }
                            self.buffer.extend(
                                plane
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.),
                            );
                        }
                        Err(Error::Again) => continue,
                        Err(e) => return Err(error(e)),
                        _ => return Err("Unexpected decoded audio frame".into()),
                    }
                }
            }
            let rate = f64::from(self.params.sample_rate);
            let channels = usize::from(self.params.channels);
            let frames = self.buffer.len() / channels;
            let skip = ((self.target - pts).max(0.) * rate)
                .round()
                .min(frames as f64) as usize;
            let end = ((self.duration - pts).max(0.) * rate)
                .round()
                .min(frames as f64) as usize;
            self.buffer.truncate(end * channels);
            self.position = skip * channels;
            if self.position < self.buffer.len() {
                return Ok(());
            }
        }
        self.buffer.clear();
        self.position = 0;
        Ok(())
    }
}

fn opus_samples(packet: &[u8]) -> Result<usize, String> {
    let toc = *packet.first().ok_or("Empty Opus packet")?;
    let config = toc >> 3;
    let samples = if config >= 16 {
        120 << (config & 3)
    } else if config >= 12 {
        480 << (config & 1)
    } else {
        [480, 960, 1920, 2880][usize::from(config & 3)]
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => usize::from(*packet.get(1).ok_or("Truncated Opus packet")? & 63),
    };
    let total = samples * frames;
    if frames == 0 || total > 5760 {
        return Err("Opus packet duration exceeds limits".into());
    }
    Ok(total)
}

impl Iterator for Audio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.position >= self.buffer.len()
            && let Err(e) = guarded(|| self.fill())
        {
            *self.error.lock().unwrap() = Some(e);
            self.next = self.packets.len();
            return None;
        }
        let sample = self.buffer.get(self.position).copied()? * self.gain;
        self.position += 1;
        if !sample.is_finite() {
            *self.error.lock().unwrap() = Some("Invalid audio sample".into());
            self.next = self.packets.len();
            self.buffer.clear();
            return None;
        }
        Some(sample.clamp(-1., 1.))
    }
}
impl rodio::Source for Audio {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> std::num::NonZero<u16> {
        std::num::NonZero::new(self.params.channels).unwrap()
    }
    fn sample_rate(&self) -> std::num::NonZero<u32> {
        std::num::NonZero::new(self.params.sample_rate).unwrap()
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        Some(std::time::Duration::from_secs_f64(self.duration))
    }
    fn try_seek(&mut self, position: std::time::Duration) -> Result<(), rodio::source::SeekError> {
        self.decoder = Self::decoder(&self.params).map_err(|e| {
            rodio::source::SeekError::Other(std::sync::Arc::new(std::io::Error::other(e)))
        })?;
        self.target = position.as_secs_f64().min(self.duration);
        let preroll = (self.target - 0.12).max(0.);
        self.next = self
            .packets
            .iter()
            .position(|p| p.pts.unwrap_or(0) as f64 / 1_000_000. >= preroll)
            .unwrap_or(self.packets.len())
            .saturating_sub(1);
        self.buffer.clear();
        self.position = 0;
        *self.error.lock().unwrap() = None;
        Ok(())
    }
}

pub(crate) fn packets(
    reader: impl Read,
    size: u64,
    media: MediaType,
) -> Result<(CodecParams, Vec<Packet>, f64, bool), String> {
    if size == 0 || size > MAX_FILE {
        return Err("WebM and newer video codecs currently require files under 32 MiB".into());
    }
    let mut bytes = Vec::new();
    reader
        .take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 != size {
        return Err("Media file size changed during loading".into());
    }
    if bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return mkv_packets(bytes, media);
    }
    let timeline = if !bytes.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) && media == MediaType::Video {
        Some(mp4_timeline(&bytes)?)
    } else {
        None
    };
    let mut formats = rff_format::FormatRegistry::new();
    rff_format_mp4::register(&mut formats);
    let name = formats
        .probe(&bytes)
        .ok_or("Unsupported media container")?
        .name;
    let mut demuxer = formats
        .open_demuxer(name, Box::new(std::io::Cursor::new(bytes)))
        .map_err(error)?;
    let streams = demuxer.read_header().map_err(error)?;
    let stream = streams
        .iter()
        .find(|s| s.media_type == media)
        .ok_or("Missing media track")?;
    if stream.extradata.len() > 1024 * 1024
        || stream.time_base.den <= 0
        || stream.time_base.num <= 0
    {
        return Err("Invalid media configuration".into());
    }
    if media == MediaType::Video {
        dimensions(stream.width, stream.height)?;
    }
    if media == MediaType::Audio && (stream.sample_rate > 192000 || stream.channels > 8) {
        return Err("Audio format exceeds limits".into());
    }
    let params = CodecParams {
        codec_id: if timeline.as_ref().is_some_and(|t| t.1) {
            CodecId::Vp9
        } else {
            stream.codec_id
        },
        width: stream.width,
        height: stream.height,
        sample_rate: stream.sample_rate,
        channels: stream.channels,
        extradata: stream.extradata.clone(),
        ..Default::default()
    };
    let scale = stream.time_base.num as f64 / stream.time_base.den as f64;
    let mut duration = stream.duration.map(|d| d as f64 * scale).unwrap_or(0.);
    let mut packets = Vec::new();
    let mut total = 0_usize;
    let mut eof = false;
    for _ in 0..1_000_000 {
        let mut packet = match demuxer.read_packet() {
            Ok(p) => p,
            Err(Error::Eof) => {
                eof = true;
                break;
            }
            Err(e) => return Err(error(e)),
        };
        if packet.data.len() > 8 * 1024 * 1024 {
            return Err("Media packet exceeds limits".into());
        }
        if packet.stream_index != stream.index {
            continue;
        }
        total = total
            .checked_add(packet.data.len())
            .ok_or("Media packet size overflow")?;
        if total > MAX_FILE as usize {
            return Err("Media packets exceed limits".into());
        }
        let pts = if let Some(timeline) = &timeline {
            *timeline
                .0
                .get(packets.len())
                .ok_or("Video timeline is shorter than its samples")?
        } else {
            packet.pts.or(packet.dts).ok_or("Missing media timestamp")? as f64 * scale
        };
        if !pts.is_finite() || pts.abs() > 604800. {
            return Err("Invalid media timestamp".into());
        }
        duration = duration.max(pts + (packet.duration as f64 * scale).max(0.));
        packet.pts = Some((pts * 1_000_000.).round() as i64);
        packets.push(packet);
    }
    if !eof || packets.is_empty() || !duration.is_finite() || duration <= 0. || duration > 604800. {
        return Err("Media duration or packet count exceeds limits".into());
    }
    Ok((
        params,
        packets,
        duration,
        streams.iter().any(|s| s.media_type == MediaType::Audio),
    ))
}

fn mp4_timeline(bytes: &[u8]) -> Result<(Vec<f64>, bool), String> {
    let mut source = std::io::Cursor::new(bytes);
    crate::decode::validate_mp4_initialization(&mut source, bytes.len() as u64)?;
    source.set_position(0);
    let mut reader = mp4::Mp4Reader::read_header(source, bytes.len() as u64).map_err(error)?;
    if reader
        .tracks()
        .values()
        .filter(|t| t.track_type().ok() == Some(mp4::TrackType::Video))
        .count()
        != 1
    {
        return Err("Choose an MP4 with one video track".into());
    }
    let track = reader
        .tracks()
        .values()
        .find(|t| t.track_type().ok() == Some(mp4::TrackType::Video))
        .ok_or("Missing video track")?;
    let id = track.track_id();
    let vp9 = track.media_type().ok() == Some(mp4::MediaType::VP9);
    let count = track.sample_count();
    let scale = f64::from(track.timescale());
    if count == 0 || count > 1_000_000 || scale <= 0. {
        return Err("Invalid video timeline".into());
    }
    let sizes = &track.trak.mdia.minf.stbl.stsz;
    if sizes.sample_size > 8 * 1024 * 1024
        || sizes.sample_sizes.iter().any(|s| *s > 8 * 1024 * 1024)
    {
        return Err("Video sample exceeds limits".into());
    }
    let mut offset = 0.;
    if let Some(edits) = track.trak.edts.as_ref().and_then(|e| e.elst.as_ref()) {
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
    let mut pts = Vec::with_capacity(count as usize);
    for i in 1..=count {
        let sample = reader
            .read_sample(id, i)
            .map_err(error)?
            .ok_or("Missing video sample")?;
        pts.push((sample.start_time as f64 + f64::from(sample.rendering_offset)) / scale - offset);
    }
    Ok((pts, vp9))
}

fn dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 || width > 1920 || height > 1920 {
        Err("Video dimensions exceed limits".into())
    } else {
        Ok(())
    }
}

fn mkv_packets(
    bytes: Vec<u8>,
    media: MediaType,
) -> Result<(CodecParams, Vec<Packet>, f64, bool), String> {
    use matroska_demuxer::{MatroskaFile, TrackType};
    validate_ebml(&bytes, 0, &mut 0)?;
    let mut file = MatroskaFile::open(std::io::Cursor::new(bytes)).map_err(error)?;
    if file.tracks().len() > 32 {
        return Err("Too many media tracks".into());
    }
    let kind = if media == MediaType::Video {
        TrackType::Video
    } else {
        TrackType::Audio
    };
    let track = file
        .tracks()
        .iter()
        .find(|t| t.track_type() == kind)
        .ok_or("Missing media track")?;
    let number = track.track_number().get();
    if track.content_encodings().is_some_and(|e| !e.is_empty()) {
        return Err("Encrypted or compressed Matroska tracks are not supported".into());
    }
    let audio = file
        .tracks()
        .iter()
        .any(|t| t.track_type() == TrackType::Audio);
    let codec_id = match track.codec_id() {
        "V_VP9" => CodecId::Vp9,
        "V_AV1" => CodecId::Avif,
        "V_MPEGH/ISO/HEVC" => CodecId::Hevc,
        "A_OPUS" => CodecId::Opus,
        "A_VORBIS" => CodecId::Vorbis,
        _ => return Err("This media codec is not supported yet".into()),
    };
    let mut params = CodecParams {
        codec_id,
        ..Default::default()
    };
    params.extradata = track.codec_private().unwrap_or_default().to_vec();
    if params.extradata.len() > 1024 * 1024 {
        return Err("Media configuration exceeds limits".into());
    }
    if let Some(v) = track.video() {
        params.width = u32::try_from(v.pixel_width().get()).map_err(error)?;
        params.height = u32::try_from(v.pixel_height().get()).map_err(error)?;
        dimensions(params.width, params.height)?;
    }
    if let Some(a) = track.audio() {
        let rate = a.sampling_frequency();
        if !(1. ..=192000.).contains(&rate) || rate.fract() != 0. || a.channels().get() > 8 {
            return Err("Audio format exceeds limits".into());
        }
        params.sample_rate = rate as u32;
        params.channels = a.channels().get() as u16;
    }
    let delay = track.codec_delay().unwrap_or(0) as f64 / 1_000_000_000.;
    let scale = file.info().timestamp_scale().get() as f64 / 1_000_000_000.;
    let duration = file.info().duration().ok_or("Media duration is missing")? * scale;
    if !duration.is_finite() || duration <= 0. || duration > 604800. || delay > duration {
        return Err("Media duration exceeds limits".into());
    }
    let hevc = if codec_id == CodecId::Hevc {
        Some(rff_format::hvc::parse_hvcc(&params.extradata).ok_or("Invalid HEVC configuration")?)
    } else {
        None
    };
    if codec_id == CodecId::Vorbis {
        params.extradata = vorbis_headers(&params.extradata)?;
    }
    let mut packets = Vec::new();
    let mut frame = matroska_demuxer::Frame::default();
    let mut count = 0;
    let mut total = 0_usize;
    while file.next_frame(&mut frame).map_err(error)? {
        count += 1;
        if count > 1_000_000 || frame.data.len() > 8 * 1024 * 1024 {
            return Err("Media packet exceeds limits".into());
        }
        if frame.track != number {
            continue;
        }
        let mut data = std::mem::take(&mut frame.data);
        if let Some(config) = &hevc {
            let mut out = if frame.is_keyframe == Some(true) {
                config.headers_annexb.clone()
            } else {
                Vec::new()
            };
            let mut offset = 0;
            while offset < data.len() {
                let length = data
                    .get(offset..offset + config.nal_len)
                    .ok_or("Truncated HEVC NAL")?
                    .iter()
                    .fold(0usize, |n, b| (n << 8) | usize::from(*b));
                offset += config.nal_len;
                let nal = data
                    .get(offset..offset.checked_add(length).ok_or("HEVC NAL size overflow")?)
                    .ok_or("Truncated HEVC NAL")?;
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(nal);
                offset += length;
                if out.len() > 8 * 1024 * 1024 {
                    return Err("HEVC packet exceeds limits".into());
                }
            }
            data = out;
        }
        total = total.checked_add(data.len()).ok_or("Media size overflow")?;
        if total > MAX_FILE as usize {
            return Err("Media packets exceed limits".into());
        }
        let pts = frame.timestamp as f64 * scale - delay;
        if !pts.is_finite() || pts.abs() > 604800. {
            return Err("Invalid media timestamp".into());
        }
        let mut packet = Packet::from_data(0, data);
        packet.pts = Some((pts * 1_000_000.).round() as i64);
        packet.flags.keyframe = frame.is_keyframe.unwrap_or(false);
        packets.push(packet);
    }
    if packets.is_empty() {
        return Err("No media packets".into());
    }
    Ok((params, packets, duration, audio))
}

fn validate_ebml(data: &[u8], depth: usize, count: &mut usize) -> Result<(), String> {
    if depth > 12 {
        return Err("Matroska metadata is nested too deeply".into());
    }
    let mut offset = 0;
    while offset < data.len() {
        *count += 1;
        if *count > 100_000 {
            return Err("Matroska element count exceeds limits".into());
        }
        let (id, _) = ebml_number(data, &mut offset, true)?;
        let (length, unknown) = ebml_number(data, &mut offset, false)?;
        let end = if unknown && id == 0x18538067 {
            data.len()
        } else if unknown {
            return Err("Unknown-sized Matroska elements are not supported".into());
        } else {
            offset
                .checked_add(usize::try_from(length).map_err(error)?)
                .filter(|end| *end <= data.len())
                .ok_or("Truncated Matroska element")?
        };
        let children = matches!(
            id,
            0x1a45dfa3
                | 0x18538067
                | 0x114d9b74
                | 0x4dbb
                | 0x1549a966
                | 0x1654ae6b
                | 0xae
                | 0xe0
                | 0xe1
                | 0x55b0
                | 0x55d0
                | 0x1f43b675
                | 0xa0
                | 0x1c53bb6b
                | 0xbb
                | 0xb7
                | 0x1254c367
                | 0x7373
                | 0x67c8
                | 0x63c0
                | 0x1941a469
                | 0x61a7
                | 0x1043a770
                | 0x45b9
                | 0xb6
                | 0x8f
                | 0x80
                | 0x6d80
                | 0x6240
                | 0x5034
                | 0x5035
        );
        if children {
            validate_ebml(&data[offset..end], depth + 1, count)?;
        } else if end - offset
            > if matches!(id, 0xa3 | 0xa1) {
                8 * 1024 * 1024
            } else {
                1024 * 1024
            }
        {
            return Err("Matroska element exceeds limits".into());
        }
        offset = end;
    }
    Ok(())
}

fn ebml_number(data: &[u8], offset: &mut usize, id: bool) -> Result<(u64, bool), String> {
    let first = *data.get(*offset).ok_or("Truncated Matroska header")?;
    let length = first.leading_zeros() as usize + 1;
    if length > if id { 4 } else { 8 } {
        return Err("Invalid Matroska integer".into());
    }
    let bytes = data
        .get(*offset..*offset + length)
        .ok_or("Truncated Matroska integer")?;
    let mut value = u64::from(if id {
        first
    } else {
        first & (0xff_u16 >> length) as u8
    });
    for byte in &bytes[1..] {
        value = (value << 8) | u64::from(*byte);
    }
    *offset += length;
    Ok((value, !id && value == (1u64 << (length * 7)) - 1))
}

fn vorbis_headers(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.first() != Some(&2) {
        return Err("Invalid Vorbis configuration".into());
    }
    let mut offset = 1;
    let mut sizes = [0usize; 3];
    for size in &mut sizes[..2] {
        loop {
            let byte = *data.get(offset).ok_or("Truncated Vorbis configuration")?;
            offset += 1;
            *size += usize::from(byte);
            if byte != 255 {
                break;
            }
        }
    }
    sizes[2] = data
        .len()
        .checked_sub(offset + sizes[0] + sizes[1])
        .ok_or("Invalid Vorbis header sizes")?;
    let mut packed = Vec::new();
    for size in sizes {
        packed.extend_from_slice(&(size as u32).to_le_bytes());
        packed.extend_from_slice(
            data.get(offset..offset + size)
                .ok_or("Truncated Vorbis configuration")?,
        );
        offset += size;
    }
    Ok(packed)
}

pub struct Video {
    params: CodecParams,
    packets: Vec<Packet>,
    decoder: Box<dyn Decoder>,
    next: usize,
    drained: bool,
    pub duration: f64,
    pub audio: bool,
}

impl Video {
    pub fn new(reader: impl Read, size: u64) -> Result<Self, String> {
        let (params, packets, duration, audio) =
            guarded(|| packets(reader, size, MediaType::Video))?;
        if !matches!(
            params.codec_id,
            CodecId::Vp9 | CodecId::Avif | CodecId::Hevc
        ) {
            return Err("This video codec is not supported yet".into());
        }
        let decoder = decoder(&params)?;
        Ok(Self {
            params,
            packets,
            decoder,
            next: 0,
            drained: false,
            duration,
            audio,
        })
    }

    pub fn seek(&mut self, seconds: f64) -> Result<(), String> {
        self.decoder = decoder(&self.params)?;
        self.next = self
            .packets
            .iter()
            .enumerate()
            .filter(|(_, p)| p.flags.keyframe && p.pts.unwrap_or(0) as f64 <= seconds * 1_000_000.)
            .map(|(i, _)| i)
            .next_back()
            .unwrap_or(0);
        self.drained = false;
        Ok(())
    }

    pub fn frame(&mut self) -> Result<Option<(f64, Pixels)>, String> {
        guarded(|| self.decode_frame())
    }

    fn decode_frame(&mut self) -> Result<Option<(f64, Pixels)>, String> {
        loop {
            match self.decoder.receive_frame() {
                Ok(Frame::Video(frame)) => {
                    return Ok(Some((
                        frame.pts.ok_or("Missing decoded timestamp")? as f64 / 1_000_000.,
                        pixels(&frame)?,
                    )));
                }
                Ok(_) => return Err("Unexpected audio frame from video decoder".into()),
                Err(Error::Eof) => return Ok(None),
                Err(Error::Again) => {}
                Err(e) => return Err(error(e)),
            }
            if let Some(packet) = self.packets.get(self.next) {
                validate_packet(&self.params, packet)?;
                self.decoder.send_packet(packet).map_err(error)?;
                self.next += 1;
            } else if !self.drained {
                self.decoder.flush();
                self.drained = true;
            } else {
                return Ok(None);
            }
        }
    }
}

fn validate_packet(params: &CodecParams, packet: &Packet) -> Result<(), String> {
    if packet.data.is_empty() {
        return Err("Empty video packet".into());
    }
    if params.codec_id == CodecId::Hevc {
        for nal in rusty_h265::nal::split_annex_b(&packet.data) {
            let header = rusty_h265::nal::NalHeader::parse(nal).ok_or("Invalid HEVC NAL")?;
            if header.nal_type == rusty_h265::nal::NalType::Sps {
                let rbsp = rusty_h265::nal::unescape(&nal[2..]);
                let sps = rusty_h265::ps::parse_sps(&rbsp.data).map_err(error)?;
                dimensions(sps.width, sps.height)?;
            }
        }
    } else if params.codec_id == CodecId::Vp9 {
        for data in vp9_frames(&packet.data)? {
            let header = rff_codec_vp9::parse_uncompressed_header(
                &mut rff_codec_vp9::BitReader::new(data),
                &[(params.width, params.height); 8],
            )
            .map_err(error)?;
            if !header.show_existing_frame {
                dimensions(header.width, header.height)?;
                if (header.width, header.height) != (params.width, params.height) {
                    return Err("Changing VP9 dimensions are not supported yet".into());
                }
            }
        }
    }
    Ok(())
}

fn vp9_frames(data: &[u8]) -> Result<Vec<&[u8]>, String> {
    let marker = *data.last().ok_or("Empty VP9 packet")?;
    if marker & 0xe0 != 0xc0 {
        return Ok(vec![data]);
    }
    let count = usize::from(marker & 7) + 1;
    let magnitude = usize::from((marker >> 3) & 3) + 1;
    let start = data
        .len()
        .checked_sub(2 + count * magnitude)
        .ok_or("Truncated VP9 index")?;
    if data[start] != marker {
        return Err("Invalid VP9 index".into());
    }
    let mut offset = 0usize;
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let mut size = 0usize;
        for j in 0..magnitude {
            size |= usize::from(data[start + 1 + i * magnitude + j]) << (8 * j);
        }
        let end = offset.checked_add(size).ok_or("VP9 size overflow")?;
        if size == 0 || end > start {
            return Err("Invalid VP9 frame size".into());
        }
        frames.push(&data[offset..end]);
        offset = end;
    }
    if offset != start {
        return Err("Invalid VP9 packet length".into());
    }
    Ok(frames)
}

fn pixels(frame: &VideoFrame) -> Result<Pixels, String> {
    dimensions(frame.width, frame.height)?;
    let (sx, sy, bits) = match frame.format {
        PixelFormat::Yuv420p => (1, 1, 8),
        PixelFormat::Yuv422p => (1, 0, 8),
        PixelFormat::Yuv444p => (0, 0, 8),
        PixelFormat::Yuv420p10 => (1, 1, 10),
        PixelFormat::Yuv422p10 => (1, 0, 10),
        PixelFormat::Yuv444p10 => (0, 0, 10),
        PixelFormat::Yuv420p12 => (1, 1, 12),
        PixelFormat::Yuv422p12 => (1, 0, 12),
        PixelFormat::Yuv444p12 => (0, 0, 12),
        _ => return Err("Unsupported decoded pixel format".into()),
    };
    let bytes = if bits == 8 { 1 } else { 2 };
    for i in 0..3 {
        let w = (frame.width as usize).div_ceil(if i == 0 { 1 } else { 1 << sx });
        let h = (frame.height as usize).div_ceil(if i == 0 { 1 } else { 1 << sy });
        let stride = *frame.strides.get(i).ok_or("Missing video plane")?;
        let required = stride.checked_mul(h).ok_or("Video plane size overflow")?;
        if stride < w * bytes || frame.planes.get(i).is_none_or(|p| p.len() < required) {
            return Err("Truncated video plane".into());
        }
    }
    let ratio = (1280. / frame.width as f64)
        .min(720. / frame.height as f64)
        .min(1.);
    let width = (frame.width as f64 * ratio).round().max(1.) as u32;
    let height = (frame.height as f64 * ratio).round().max(1.) as u32;
    let sample = |plane: usize, x: usize, y: usize| -> i32 {
        let offset = y * frame.strides[plane] + x * bytes;
        let p = &frame.planes[plane];
        if bytes == 1 {
            i32::from(p[offset])
        } else {
            i32::from(u16::from_le_bytes([p[offset], p[offset + 1]]) >> (bits - 8))
        }
    };
    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            let px = (u64::from(x) * u64::from(frame.width) / u64::from(width)) as usize;
            let py = (u64::from(y) * u64::from(frame.height) / u64::from(height)) as usize;
            let c = sample(0, px, py) - 16;
            let d = sample(1, px >> sx, py >> sy) - 128;
            let e = sample(2, px >> sx, py >> sy) - 128;
            rgba.extend_from_slice(&[
                ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8,
                ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8,
                ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8,
                255,
            ]);
        }
    }
    Ok(Pixels {
        rgba,
        width,
        height,
    })
}

struct Av1 {
    decoder: rusty_av1d::Decoder,
    pending: bool,
}

impl Av1 {
    fn new() -> Result<Self, String> {
        let mut settings = rusty_av1d::Settings::new();
        settings.set_n_threads(1);
        settings.set_max_frame_delay(1);
        settings.set_frame_size_limit(1920 * 1920);
        Ok(Self {
            decoder: rusty_av1d::Decoder::with_settings(&settings).map_err(error)?,
            pending: false,
        })
    }
}
impl Decoder for Av1 {
    fn send_packet(&mut self, packet: &Packet) -> rff_core::Result<()> {
        if packet.data.is_empty() || self.pending {
            return Err(Error::InvalidData("AV1 input was not consumed".into()));
        }
        match self.decoder.send_data(
            packet.data.clone().into_boxed_slice(),
            None,
            packet.pts,
            None,
        ) {
            Ok(()) => Ok(()),
            Err(rusty_av1d::Rav1dError::TryAgain) => {
                self.pending = true;
                Ok(())
            }
            Err(e) => Err(Error::InvalidData(e.to_string())),
        }
    }
    fn receive_frame(&mut self) -> rff_core::Result<Frame> {
        if self.pending {
            match self.decoder.send_pending_data() {
                Ok(()) => self.pending = false,
                Err(rusty_av1d::Rav1dError::TryAgain) => {}
                Err(e) => return Err(Error::InvalidData(e.to_string())),
            }
        }
        let picture = self.decoder.get_picture().map_err(|e| {
            if e == rusty_av1d::Rav1dError::TryAgain {
                Error::Again
            } else {
                Error::InvalidData(e.to_string())
            }
        })?;
        let format = match (picture.pixel_layout(), picture.bit_depth()) {
            (rusty_av1d::PixelLayout::I420, 8) => PixelFormat::Yuv420p,
            (rusty_av1d::PixelLayout::I420, 10) => PixelFormat::Yuv420p10,
            (rusty_av1d::PixelLayout::I420, 12) => PixelFormat::Yuv420p12,
            _ => return Err(Error::Unsupported("AV1 requires 4:2:0 chroma".into())),
        };
        let mut planes = Vec::new();
        let mut strides = Vec::new();
        for component in [
            rusty_av1d::PlanarImageComponent::Y,
            rusty_av1d::PlanarImageComponent::U,
            rusty_av1d::PlanarImageComponent::V,
        ] {
            planes.push(picture.plane(component).to_vec());
            strides.push(picture.stride(component) as usize);
        }
        Ok(Frame::Video(VideoFrame {
            width: picture.width(),
            height: picture.height(),
            format,
            planes,
            strides,
            pts: picture.timestamp(),
        }))
    }
}

#[cfg(test)]
mod limits {
    use super::*;

    #[test]
    fn rejects_oversized_files_before_reading() {
        assert!(packets(std::io::empty(), MAX_FILE + 1, MediaType::Video).is_err());
        assert!(dimensions(1921, 1080).is_err());
        assert!(dimensions(1920, 0).is_err());
    }

    #[test]
    fn rejects_invalid_container_lengths_and_excessive_nesting() {
        for bytes in [&[0u8][..], &[0xa3, 0x88, 0][..], &[0xa3, 0xff][..]] {
            assert!(validate_ebml(bytes, 0, &mut 0).is_err());
        }
        assert!(validate_ebml(&[], 13, &mut 0).is_err());
        assert!(validate_ebml(&[0xec, 0x80], 0, &mut 100_000).is_err());
        assert!(validate_ebml(&[0xec, 0x80], 0, &mut 0).is_ok());
    }

    #[test]
    fn rejects_audio_configuration_and_packet_size_abuse() {
        assert!(opus_samples(&[]).is_err());
        assert!(opus_samples(&[3, 63]).is_err());
        assert!(opus_samples(&[3, 0]).is_err());
        assert!(vorbis_headers(&[2, 255]).is_err());
        assert!(vorbis_headers(&[2, 20, 30, 0]).is_err());
    }

    #[test]
    fn rejects_invalid_vp9_superframe_offsets() {
        assert!(vp9_frames(&[0xc1]).is_err());
        assert!(vp9_frames(&[1, 0xc1, 100, 100, 0xc1]).is_err());
    }

    #[test]
    fn rejects_video_plane_overflow() {
        let frame = VideoFrame {
            width: 160,
            height: 90,
            format: PixelFormat::Yuv420p,
            planes: vec![vec![]; 3],
            strides: vec![usize::MAX; 3],
            pts: Some(0),
        };
        assert!(pixels(&frame).is_err());
    }

    #[test]
    fn hevc_horizontal_edge_at_left_border_has_valid_bounds() {
        let mut actual = vec![128u16; 16 * 16];
        let mut expected = actual.clone();
        rusty_h265::accel::deblock::luma_edge_scalar(
            &mut expected,
            16,
            0,
            8,
            1,
            32,
            4,
            false,
            false,
            255,
        );
        rusty_h265::accel::deblock::luma_edge(&mut actual, 16, 0, 8, 1, 32, 4, false, false, 255);
        assert_eq!(actual, expected);
    }
}
