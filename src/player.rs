use crate::{Pixels, decode::Video, http::RemoteFile, providers};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

#[derive(Default, Debug)]
pub struct Snapshot {
    pub status: Status,
    pub title: String,
    pub provider: String,
    pub error: String,
    pub position: f64,
    pub duration: f64,
    pub pixels: Option<Pixels>,
    pub generation: u64,
}

struct Controls {
    pending: Option<String>,
    paused: bool,
    volume: f32,
    seek: Option<f64>,
    cancel: Arc<AtomicBool>,
    shutdown: bool,
    generation: u64,
}
struct Shared {
    controls: Mutex<Controls>,
    changed: Condvar,
    snapshot: Mutex<Snapshot>,
}
pub struct Player {
    shared: Arc<Shared>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            controls: Mutex::new(Controls {
                pending: None,
                paused: false,
                volume: 1.,
                seek: None,
                cancel: Arc::default(),
                shutdown: false,
                generation: 0,
            }),
            changed: Condvar::new(),
            snapshot: Mutex::default(),
        });
        let worker = shared.clone();
        std::thread::spawn(move || run(worker));
        Self { shared }
    }
    pub fn load(&self, input: String) {
        let mut controls = self.shared.controls.lock().unwrap();
        controls.cancel.store(true, Ordering::Relaxed);
        controls.cancel = Arc::default();
        controls.pending = Some(input);
        controls.paused = false;
        controls.seek = None;
        controls.generation = controls.generation.wrapping_add(1);
        *self.shared.snapshot.lock().unwrap() = Snapshot {
            status: Status::Loading,
            generation: controls.generation,
            ..Default::default()
        };
        self.shared.changed.notify_one();
    }
    pub fn pause(&self, paused: bool) {
        self.shared.controls.lock().unwrap().paused = paused;
    }
    pub fn seek(&self, seconds: f64) {
        if seconds.is_finite() {
            self.shared.controls.lock().unwrap().seek = Some(seconds.max(0.));
        }
    }
    pub fn set_volume(&self, volume: f32) {
        if volume.is_finite() {
            self.shared.controls.lock().unwrap().volume = volume.clamp(0., 1.);
        }
    }
    pub fn stop(&self) {
        let mut controls = self.shared.controls.lock().unwrap();
        controls.cancel.store(true, Ordering::Relaxed);
        controls.pending = None;
        controls.generation = controls.generation.wrapping_add(1);
        *self.shared.snapshot.lock().unwrap() = Snapshot::default();
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut state = self.shared.snapshot.lock().unwrap();
        Snapshot {
            status: state.status,
            title: state.title.clone(),
            provider: state.provider.clone(),
            error: state.error.clone(),
            position: state.position,
            duration: state.duration,
            pixels: state.pixels.take(),
            generation: state.generation,
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let mut controls = self.shared.controls.lock().unwrap();
        controls.shutdown = true;
        controls.cancel.store(true, Ordering::Relaxed);
        self.shared.changed.notify_one();
    }
}

fn run(shared: Arc<Shared>) {
    loop {
        let (input, cancel, generation) = {
            let mut controls = shared.controls.lock().unwrap();
            while controls.pending.is_none() && !controls.shutdown {
                controls = shared.changed.wait(controls).unwrap();
            }
            if controls.shutdown {
                return;
            }
            (
                controls.pending.take().unwrap(),
                controls.cancel.clone(),
                controls.generation,
            )
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            playback(&input, &cancel, generation, &shared)
        }));
        if cancel.load(Ordering::Relaxed) {
            continue;
        }
        if let Err(error) =
            result.unwrap_or_else(|_| Err("The decoder could not read this video".into()))
        {
            let mut snapshot = shared.snapshot.lock().unwrap();
            if snapshot.generation == generation {
                snapshot.error = error;
                snapshot.status = Status::Failed;
            }
        }
    }
}

pub enum MediaReader {
    Remote(RemoteFile),
    Local {
        file: File,
        path: PathBuf,
        size: u64,
    },
}
impl MediaReader {
    pub fn open(input: &str, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        if input.starts_with("https://") {
            return RemoteFile::open(input, cancel).map(Self::Remote);
        }
        if input.contains("://") {
            return Err("Use an HTTPS video URL or a local file".into());
        }
        let path = PathBuf::from(input);
        let file = File::open(&path).map_err(|_| "Could not open that file")?;
        let metadata = file.metadata().map_err(|_| "Could not read that file")?;
        if !metadata.is_file() || metadata.len() > 2 * 1024 * 1024 * 1024 {
            return Err("Choose a video file smaller than 2 GB".into());
        }
        Ok(Self::Local {
            file,
            path,
            size: metadata.len(),
        })
    }
    pub fn size(&self) -> u64 {
        match self {
            Self::Remote(file) => file.size,
            Self::Local { size, .. } => *size,
        }
    }
    fn duplicate(&self) -> Result<Self, String> {
        match self {
            Self::Remote(file) => Ok(Self::Remote(file.clone())),
            Self::Local { path, .. } => {
                Self::open(path.to_str().ok_or("Invalid file path")?, Arc::default())
            }
        }
    }
}
impl Read for MediaReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Remote(file) => file.read(bytes),
            Self::Local { file, .. } => file.read(bytes),
        }
    }
}
impl Seek for MediaReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            Self::Remote(file) => file.seek(pos),
            Self::Local { file, .. } => file.seek(pos),
        }
    }
}

fn playback(
    input: &str,
    cancel: &Arc<AtomicBool>,
    generation: u64,
    shared: &Shared,
) -> Result<(), String> {
    let resolved = providers::resolve(input)?;
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let source = MediaReader::open(&resolved.video, cancel.clone())?;
    let size = source.size();
    let audio_source = if let Some(audio) = &resolved.audio {
        Some(MediaReader::open(audio, cancel.clone())?)
    } else {
        None
    };
    let mut video = Video::new(source.duplicate()?, size)?;
    let mut next = video.frame()?;
    let audio = if video.audio || audio_source.is_some() {
        let source = audio_source.unwrap_or(source);
        let size = source.size();
        let mut output = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|_| "No audio output device is available")?;
        output.log_on_drop(false);
        let decoder = rodio::Decoder::builder()
            .with_data(source)
            .with_byte_len(size)
            .with_hint("mp4")
            .with_seekable(true)
            .build()
            .map_err(|_| "This audio codec is not supported yet")?;
        let player = rodio::Player::connect_new(output.mixer());
        player.pause();
        player.append(decoder);
        Some((output, player))
    } else {
        None
    };
    let mut time = 0.;
    let mut previous = Instant::now();
    let mut ended = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let (paused, volume, seek) = {
            let mut controls = shared.controls.lock().unwrap();
            (controls.paused, controls.volume, controls.seek.take())
        };
        if let Some(seek) = seek {
            time = seek.clamp(0., video.duration);
            if let Some((_, player)) = &audio {
                player.pause();
            }
            video.seek(time)?;
            next = video.frame()?;
            while next.as_ref().is_some_and(|(pts, _)| *pts < time) {
                if cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                next = video.frame()?;
            }
            if let Some((_, player)) = &audio {
                player
                    .try_seek(Duration::from_secs_f64(time))
                    .map_err(|_| "Could not seek the audio")?;
            }
            ended = false;
            previous = Instant::now();
            if let Some((_, pixels)) = next.take() {
                let mut snapshot = shared.snapshot.lock().unwrap();
                if snapshot.generation == generation {
                    snapshot.pixels = Some(pixels);
                }
                drop(snapshot);
                next = video.frame()?;
            }
        }
        if let Some((_, player)) = &audio {
            player.set_volume(volume);
            if paused || ended {
                player.pause();
            } else {
                player.play();
            }
            if !paused && !ended {
                if player.empty() {
                    time += previous.elapsed().as_secs_f64();
                } else {
                    time = player.get_pos().as_secs_f64();
                }
            }
        } else if !paused && !ended {
            time += previous.elapsed().as_secs_f64();
        }
        previous = Instant::now();
        let mut pixels = None;
        while next.as_ref().is_some_and(|(pts, _)| *pts <= time + 0.01) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            pixels = next.take().map(|(_, pixels)| pixels);
            next = video.frame()?;
        }
        if next.is_none() && time >= video.duration - 0.05 {
            ended = true;
        }
        let mut state = shared.snapshot.lock().unwrap();
        if state.generation != generation {
            return Ok(());
        }
        if pixels.is_some() {
            state.pixels = pixels;
        }
        state.position = time.min(video.duration);
        state.duration = video.duration;
        state.title.clone_from(&resolved.title);
        state.provider.clone_from(&resolved.provider);
        state.status = if ended {
            Status::Ended
        } else if paused {
            Status::Paused
        } else {
            Status::Playing
        };
        drop(state);
        std::thread::sleep(Duration::from_millis(8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_invalidates_pending_playback() {
        let player = Player::new();
        player.load("missing.mp4".into());
        player.stop();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(player.snapshot().status, Status::Idle);
    }
    #[test]
    fn invalid_controls_cannot_poison_playback() {
        let player = Player::new();
        player.seek(f64::NAN);
        player.set_volume(f32::INFINITY);
        let controls = player.shared.controls.lock().unwrap();
        assert!(controls.seek.is_none());
        assert_eq!(controls.volume, 1.);
    }
}
