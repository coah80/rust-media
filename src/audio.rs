use crate::player::MediaReader;
use rodio::Source;
use std::{
    io,
    num::NonZero,
    sync::{Arc, Mutex},
    time::Duration,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_AAC, Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    probe::Hint,
    units::TimeBase,
};

impl MediaSource for MediaReader {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.size())
    }
}

pub struct Audio {
    inner: Box<dyn Source<Item = f32> + Send>,
    source_error: Arc<Mutex<Option<String>>>,
    pub error: Arc<Mutex<Option<String>>>,
}

impl Audio {
    pub fn new(mut source: MediaReader) -> Result<Self, String> {
        use std::io::{Read, Seek, SeekFrom};
        let mut magic = [0; 4];
        source
            .read_exact(&mut magic)
            .map_err(|_| "Could not read audio header")?;
        source
            .seek(SeekFrom::Start(0))
            .map_err(|_| "Could not seek audio")?;
        if magic == [0x1a, 0x45, 0xdf, 0xa3] {
            let audio = crate::modern::Audio::new(source)?;
            Ok(Self {
                source_error: audio.error.clone(),
                error: audio.error.clone(),
                inner: Box::new(audio),
            })
        } else {
            let audio = Aac::new(source)?;
            Ok(Self {
                source_error: audio.error.clone(),
                error: audio.error.clone(),
                inner: Box::new(audio),
            })
        }
    }
}
impl Iterator for Audio {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        let sample = self.inner.next();
        if sample.is_none() {
            let error = self.source_error.lock().unwrap().clone();
            *self.error.lock().unwrap() = error;
        }
        sample
    }
}
impl Source for Audio {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> NonZero<u16> {
        self.inner.channels()
    }
    fn sample_rate(&self) -> NonZero<u32> {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(position)?;
        *self.error.lock().unwrap() = None;
        Ok(())
    }
}

struct Aac {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    rate: NonZero<u32>,
    channels: NonZero<u16>,
    time_base: TimeBase,
    duration: Option<Duration>,
    buffer: Vec<f32>,
    position: usize,
    seek_target: Option<f64>,
    ended: bool,
    format_known: bool,
    pub error: Arc<Mutex<Option<String>>>,
}
impl Aac {
    pub fn new(source: MediaReader) -> Result<Self, String> {
        let stream = MediaSourceStream::new(Box::new(source), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("mp4");
        let format = symphonia::default::get_probe()
            .format(&hint, stream, &Default::default(), &Default::default())
            .map_err(|_| "Could not read the audio container")?
            .format;
        let track = format
            .tracks()
            .iter()
            .find(|track| track.codec_params.codec == CODEC_TYPE_AAC)
            .ok_or("This audio codec is not supported yet")?;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())
            .map_err(|_| "Could not start the AAC decoder")?;
        let rate = NonZero::new(1).unwrap();
        let channels = NonZero::new(1).unwrap();
        let time_base = track.codec_params.time_base.ok_or("Missing audio clock")?;
        let duration = track.codec_params.n_frames.map(|frames| {
            let time = time_base.calc_time(frames);
            Duration::from_secs_f64(time.seconds as f64 + time.frac)
        });
        let mut audio = Self {
            track: track.id,
            format,
            decoder,
            rate,
            channels,
            time_base,
            duration,
            buffer: Vec::new(),
            position: 0,
            seek_target: None,
            ended: false,
            format_known: false,
            error: Arc::default(),
        };
        audio.fill().map_err(|_| "Could not decode the audio")?;
        if audio.buffer.is_empty() {
            return Err("The video contains no audio samples".into());
        }
        Ok(audio)
    }
    fn fill(&mut self) -> Result<(), Error> {
        self.buffer.clear();
        self.position = 0;
        for _ in 0..100_000 {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(Error::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.ended = true;
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            if packet.track_id() != self.track {
                continue;
            }
            let time = self.time_base.calc_time(packet.ts());
            let seconds = time.seconds as f64 + time.frac;
            let decoded = self.decoder.decode(&packet)?;
            if !self.format_known {
                self.rate = NonZero::new(decoded.spec().rate)
                    .ok_or(Error::DecodeError("invalid sample rate"))?;
                self.channels = NonZero::new(decoded.spec().channels.count() as u16)
                    .ok_or(Error::DecodeError("invalid channels"))?;
                if self.rate.get() > 192_000 || self.channels.get() > 8 {
                    return Err(Error::LimitError("audio format exceeds limits"));
                }
                self.format_known = true;
            }
            if decoded.spec().rate != self.rate.get()
                || decoded.spec().channels.count() != usize::from(self.channels.get())
                || decoded.capacity() > 65536
            {
                return Err(Error::Unsupported("audio format changed"));
            }
            let frames = decoded.frames();
            let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
            samples.copy_interleaved_ref(decoded);
            let skip = self
                .seek_target
                .map(|target| {
                    ((target - seconds).max(0.) * f64::from(self.rate.get())).round() as usize
                })
                .unwrap_or(0);
            if skip >= frames {
                continue;
            }
            self.buffer
                .extend_from_slice(&samples.samples()[skip * usize::from(self.channels.get())..]);
            self.seek_target = None;
            return Ok(());
        }
        Err(Error::LimitError("too many non-audio packets"))
    }
}
impl Iterator for Aac {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.position >= self.buffer.len() {
            if self.ended {
                return None;
            }
            if self.fill().is_err() {
                *self.error.lock().unwrap() = Some("Audio playback was interrupted".into());
                self.ended = true;
                return None;
            }
        }
        let sample = self.buffer.get(self.position).copied()?;
        self.position += 1;
        Some(sample)
    }
}
impl Source for Aac {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> NonZero<u16> {
        self.channels
    }
    fn sample_rate(&self) -> NonZero<u32> {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        let position = position.min(self.duration.unwrap_or(position));
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time: position.as_secs_f64().into(),
                    track_id: Some(self.track),
                },
            )
            .map_err(|_| {
                rodio::source::SeekError::Other(Arc::new(io::Error::other("Could not seek audio")))
            })?;
        self.decoder.reset();
        self.ended = false;
        self.buffer.clear();
        self.position = 0;
        self.seek_target = Some(position.as_secs_f64());
        *self.error.lock().unwrap() = None;
        Ok(())
    }
}
