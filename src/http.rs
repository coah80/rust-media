use std::{
    collections::BTreeMap,
    io::{self, Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

const BLOCK: u64 = 512 * 1024;
const MAX_SIZE: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Default)]
struct Cache {
    blocks: Mutex<BTreeMap<u64, Arc<[u8]>>>,
    size: AtomicU64,
    prefix: AtomicU64,
}

impl Cache {
    fn get(&self, position: u64) -> Option<(u64, Arc<[u8]>)> {
        let blocks = self.blocks.lock().unwrap();
        let (&start, bytes) = blocks.range(..=position).next_back()?;
        (position < start + bytes.len() as u64).then(|| (start, bytes.clone()))
    }

    fn store(&self, start: u64, bytes: Arc<[u8]>, size: u64) {
        self.size.store(size, Ordering::Relaxed);
        let mut blocks = self.blocks.lock().unwrap();
        blocks.entry(start).or_insert(bytes);
        let mut prefix = 0;
        for (&start, bytes) in blocks.iter() {
            if start > prefix {
                break;
            }
            prefix = prefix.max(start.saturating_add(bytes.len() as u64));
        }
        self.prefix.store(prefix.min(size), Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct BufferProgress(Arc<Cache>);

impl BufferProgress {
    pub fn fraction(&self) -> f64 {
        let size = self.0.size.load(Ordering::Relaxed);
        if size == 0 {
            0.
        } else {
            self.0.prefix.load(Ordering::Relaxed) as f64 / size as f64
        }
    }
}

pub fn allowed(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.host_str().is_some_and(|host| {
                matches!(
                    host,
                    "video.twimg.com"
                        | "cdn.discordapp.com"
                        | "media.discordapp.net"
                        | "images-ext-1.discordapp.net"
                        | "images-ext-2.discordapp.net"
                        | "media.tenor.com"
                        | "download.blender.org"
                        | "media.w3.org"
                ) || host.ends_with(".googlevideo.com")
            })
    })
}

#[derive(Clone)]
pub struct RemoteFile {
    client: reqwest::blocking::Client,
    url: String,
    cancel: Arc<AtomicBool>,
    position: u64,
    pub size: u64,
    start: u64,
    bytes: Arc<[u8]>,
    cache: Option<Arc<Cache>>,
}

impl RemoteFile {
    pub fn open(url: &str, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        Self::open_inner(url, cancel, None)
    }

    pub fn open_progressive(
        url: &str,
        cancel: Arc<AtomicBool>,
        max_size: u64,
    ) -> Result<Self, String> {
        let cache = Arc::new(Cache::default());
        let file = Self::open_inner(url, cancel, Some(cache))?;
        if file.size > max_size {
            return Err("Video exceeds the progressive loading limit".into());
        }
        Ok(file)
    }

    pub fn start_prefetch(&self) {
        let mut download = self.clone();
        std::thread::spawn(move || {
            let mut output = [0; 64 * 1024];
            loop {
                match download.read(&mut output) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
            }
        });
    }

    fn open_inner(
        url: &str,
        cancel: Arc<AtomicBool>,
        cache: Option<Arc<Cache>>,
    ) -> Result<Self, String> {
        if !allowed(url) {
            return Err("Unsupported video address".into());
        }
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| "Could not start video loading")?;
        let mut file = Self {
            client,
            url: url.into(),
            cancel,
            position: 0,
            size: 0,
            start: 0,
            bytes: Arc::from([]),
            cache,
        };
        file.load(0)
            .map_err(|_| "Video unavailable. Retry or open the original link")?;
        Ok(file)
    }

    pub fn progress(&self) -> Option<BufferProgress> {
        self.cache.clone().map(BufferProgress)
    }

    fn load(&mut self, start: u64) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if let Some((actual_start, bytes)) = self.cache.as_ref().and_then(|cache| cache.get(start))
        {
            self.start = actual_start;
            self.bytes = bytes;
            return Ok(());
        }
        let end = start.saturating_add(BLOCK - 1).min(if self.size == 0 {
            MAX_SIZE
        } else {
            self.size - 1
        });
        let mut failure = None;
        for attempt in 0..3 {
            match self.request_block(start, end) {
                Ok((actual_start, size, bytes)) => {
                    if size == 0 || size > MAX_SIZE || (self.size != 0 && self.size != size) {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    let bytes: Arc<[u8]> = bytes.into();
                    if let Some(cache) = &self.cache {
                        cache.store(actual_start, bytes.clone(), size);
                    }
                    self.size = size;
                    self.start = actual_start;
                    self.bytes = bytes;
                    return Ok(());
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Other
                            | io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                    ) =>
                {
                    failure = Some(error);
                    std::thread::sleep(Duration::from_millis(50 * (attempt + 1)));
                }
                Err(error) => return Err(error),
            }
        }
        Err(failure.unwrap_or_else(|| io::Error::other("Video range request failed")))
    }

    fn request_block(&self, start: u64, end: u64) -> io::Result<(u64, u64, Vec<u8>)> {
        let mut url = self.url.clone();
        for _ in 0..5 {
            if !allowed(&url) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let response = self
                .client
                .get(&url)
                .header("Range", format!("bytes={start}-{end}"))
                .header("Accept-Encoding", "identity")
                .send()
                .map_err(io::Error::other)?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or(io::ErrorKind::InvalidData)?;
                url = reqwest::Url::parse(&url)
                    .map_err(io::Error::other)?
                    .join(location)
                    .map_err(io::Error::other)?
                    .into();
                continue;
            }
            let partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
            if !response.status().is_success() {
                let status = response.status();
                let kind = if status.is_server_error() {
                    io::ErrorKind::Other
                } else {
                    io::ErrorKind::PermissionDenied
                };
                return Err(io::Error::new(kind, format!("HTTP {status}")));
            }
            let (actual_start, size, limit) = if partial {
                let range = response
                    .headers()
                    .get("content-range")
                    .and_then(|value| value.to_str().ok())
                    .ok_or(io::ErrorKind::InvalidData)?;
                let (bounds, total) = range
                    .strip_prefix("bytes ")
                    .and_then(|value| value.split_once('/'))
                    .ok_or(io::ErrorKind::InvalidData)?;
                let (first, last) = bounds.split_once('-').ok_or(io::ErrorKind::InvalidData)?;
                let first = first.parse::<u64>().map_err(io::Error::other)?;
                let last = last.parse::<u64>().map_err(io::Error::other)?;
                let total = total.parse::<u64>().map_err(io::Error::other)?;
                if first != start || last < first || last > end || last >= total {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                (first, total, last - first + 1)
            } else {
                let size = response
                    .content_length()
                    .ok_or(io::ErrorKind::InvalidData)?;
                if size > 32 * 1024 * 1024 {
                    return Err(io::Error::other("Server does not support video seeking"));
                }
                (0, size, size)
            };
            let mut bytes = Vec::new();
            response.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != limit || self.cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            return Ok((actual_start, size, bytes));
        }
        Err(io::Error::other("Too many video redirects"))
    }
}

impl Read for RemoteFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if output.is_empty() || self.position >= self.size {
            return Ok(0);
        }
        if self.position < self.start || self.position >= self.start + self.bytes.len() as u64 {
            self.load(self.position / BLOCK * BLOCK)?;
        }
        let offset = (self.position - self.start) as usize;
        let length = output.len().min(self.bytes.len() - offset);
        output[..length].copy_from_slice(&self.bytes[offset..offset + length]);
        self.position += length as u64;
        Ok(length)
    }
}

impl Seek for RemoteFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(value) => i128::from(value),
            SeekFrom::End(value) => i128::from(self.size) + i128::from(value),
            SeekFrom::Current(value) => i128::from(self.position) + i128::from(value),
        };
        if !(0..=i128::from(self.size)).contains(&next) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        self.position = next as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_progress_stops_at_gaps() {
        let cache = Arc::new(Cache::default());
        let progress = BufferProgress(cache.clone());
        cache.store(4, Arc::from([0; 4]), 12);
        assert_eq!(progress.fraction(), 0.);
        cache.store(0, Arc::from([0; 4]), 12);
        assert!((progress.fraction() - 8. / 12.).abs() < f64::EPSILON);
        cache.store(8, Arc::from([0; 4]), 12);
        assert_eq!(progress.fraction(), 1.);
    }

    #[test]
    fn media_destinations_are_restricted() {
        for url in [
            "https://video.twimg.com/video.mp4",
            "https://cdn.discordapp.com/attachments/a.mp4",
            "https://rr1.googlevideo.com/videoplayback",
        ] {
            assert!(super::allowed(url));
        }
        for url in [
            "file:///etc/passwd",
            "http://video.twimg.com/a",
            "https://localhost/a",
            "https://video.twimg.com.evil.test/a",
            "https://googlevideo.com.evil.test/a",
            "https://user:pass@video.twimg.com/a",
            "https://video.twimg.com:444/a",
        ] {
            assert!(!super::allowed(url));
        }
    }
}
