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
    client: reqwest::Client,
    url: String,
    cancel: Arc<AtomicBool>,
    position: u64,
    pub size: u64,
    start: u64,
    bytes: Arc<[u8]>,
    cache: Option<Arc<Cache>>,
    max_size: u64,
}

impl RemoteFile {
    pub fn open(url: &str, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        Self::open_inner(url, cancel, None, MAX_SIZE)
    }

    pub fn open_progressive(
        url: &str,
        cancel: Arc<AtomicBool>,
        max_size: u64,
    ) -> Result<Self, String> {
        if max_size == 0 || max_size > MAX_SIZE {
            return Err("Invalid progressive loading limit".into());
        }
        let cache = Arc::new(Cache::default());
        Self::open_inner(url, cancel, Some(cache), max_size)
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
        max_size: u64,
    ) -> Result<Self, String> {
        if !allowed(url) {
            return Err("Unsupported video address".into());
        }
        let client = reqwest::Client::builder()
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
            max_size,
        };
        file.load(0)
            .map_err(|_| "Video unavailable. Retry or open the original link")?;
        Ok(file)
    }

    pub fn progress(&self) -> Option<BufferProgress> {
        self.cache.clone().map(BufferProgress)
    }

    fn load(&mut self, position: u64) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if let Some((actual_start, bytes)) =
            self.cache.as_ref().and_then(|cache| cache.get(position))
        {
            self.start = actual_start;
            self.bytes = bytes;
            return Ok(());
        }
        let start = position / BLOCK * BLOCK;
        let end = start.saturating_add(BLOCK - 1).min(if self.size == 0 {
            self.max_size - 1
        } else {
            self.size - 1
        });
        let (actual_start, size, bytes) =
            retry_request(&self.cancel, || self.request_block(start, end))?;
        if size == 0 || size > self.max_size || (self.size != 0 && self.size != size) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let bytes: Arc<[u8]> = bytes.into();
        if let Some(cache) = &self.cache {
            cache.store(actual_start, bytes.clone(), size);
        }
        self.size = size;
        self.start = actual_start;
        self.bytes = bytes;
        Ok(())
    }

    fn request_block(&self, start: u64, end: u64) -> io::Result<(u64, u64, Vec<u8>)> {
        self.request_block_from(&self.url, start, end, false)
    }

    fn request_block_from(
        &self,
        initial_url: &str,
        start: u64,
        end: u64,
        allow_initial: bool,
    ) -> io::Result<(u64, u64, Vec<u8>)> {
        crate::request::run(self.request_block_from_async(initial_url, start, end, allow_initial))
            .map_err(io::Error::other)?
    }

    async fn request_block_from_async(
        &self,
        initial_url: &str,
        start: u64,
        end: u64,
        allow_initial: bool,
    ) -> io::Result<(u64, u64, Vec<u8>)> {
        let mut url = initial_url.to_owned();
        for redirect in 0..5 {
            if (!allow_initial || redirect > 0) && !allowed(&url) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let request = self
                .client
                .get(&url)
                .header("Range", format!("bytes={start}-{end}"))
                .header("Accept-Encoding", "identity");
            let response = tokio::select! {
                biased;
                _ = cancelled(&self.cancel) => return Err(io::ErrorKind::Interrupted.into()),
                response = request.send() => response.map_err(io::Error::other)?,
            };
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
                enforce_size_limit(total, self.max_size)?;
                (first, total, last - first + 1)
            } else {
                let size = response
                    .content_length()
                    .ok_or(io::ErrorKind::InvalidData)?;
                enforce_size_limit(size, self.max_size)?;
                if size > 32 * 1024 * 1024 {
                    return Err(io::Error::other("Server does not support video seeking"));
                }
                (0, size, size)
            };
            let bytes = read_body(response, limit + 1, &self.cancel).await?;
            if bytes.len() as u64 != limit || self.cancel.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            return Ok((actual_start, size, bytes));
        }
        Err(io::Error::other("Too many video redirects"))
    }
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn read_body(
    mut response: reqwest::Response,
    limit: u64,
    cancel: &AtomicBool,
) -> io::Result<Vec<u8>> {
    let limit = usize::try_from(limit).map_err(io::Error::other)?;
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancelled(cancel) => return Err(io::ErrorKind::Interrupted.into()),
            chunk = response.chunk() => chunk.map_err(io::Error::other)?,
        };
        let Some(chunk) = chunk else {
            return Ok(bytes);
        };
        if chunk.len() > limit - bytes.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        bytes.extend_from_slice(&chunk);
    }
}

fn enforce_size_limit(size: u64, max_size: u64) -> io::Result<()> {
    if size == 0 || size > max_size {
        Err(io::ErrorKind::InvalidData.into())
    } else {
        Ok(())
    }
}

fn retry_request<T>(
    cancel: &AtomicBool,
    mut request: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut failure = None;
    for attempt in 0..3 {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        match request() {
            Ok(value) => return Ok(value),
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
                let wait = Duration::from_millis(50 * (attempt + 1));
                let began = std::time::Instant::now();
                while began.elapsed() < wait {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(io::ErrorKind::Interrupted.into());
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(failure.unwrap_or_else(|| io::Error::other("Video range request failed")))
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
            self.load(self.position)?;
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

    #[test]
    fn macroscope_progressive_size_limit_is_checked_before_body() {
        assert!(enforce_size_limit(1024, 1024).is_ok());
        assert_eq!(
            enforce_size_limit(1025, 1024).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn macroscope_cancellation_stops_transient_retries() {
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let error = retry_request(&cancel, || {
            calls += 1;
            cancel.store(true, Ordering::Relaxed);
            Err::<(), _>(io::ErrorKind::TimedOut.into())
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(calls, 1);
    }

    #[test]
    fn macroscope_short_cached_block_does_not_end_stream() {
        let cache = Arc::new(Cache::default());
        cache.store(0, Arc::from([0, 1]), 4);
        cache.store(2, Arc::from([2, 3]), 4);
        let mut file = RemoteFile {
            client: reqwest::Client::new(),
            url: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            position: 2,
            size: 4,
            start: 0,
            bytes: Arc::from([0, 1]),
            cache: Some(cache),
            max_size: 4,
        };
        let mut output = [0; 2];
        assert_eq!(file.read(&mut output).unwrap(), 2);
        assert_eq!(output, [2, 3]);
    }

    #[test]
    fn macroscope_stalled_range_body_cancels_promptly() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, mpsc},
            time::Instant,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (started_tx, started_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            socket
                .write_all(
                    b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 0-3/4\r\n\r\nx",
                )
                .unwrap();
            started_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_secs(2));
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let file = RemoteFile {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            url: url.clone(),
            cancel: cancel.clone(),
            position: 0,
            size: 0,
            start: 0,
            bytes: Arc::from([]),
            cache: None,
            max_size: 4,
        };
        let worker_cancel = cancel.clone();
        let worker_url = url.clone();
        let worker = std::thread::spawn(move || {
            retry_request(&worker_cancel, || {
                file.request_block_from(&worker_url, 0, 3, true)
            })
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let began = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(began.elapsed() < Duration::from_millis(500));
        server.join().unwrap();
    }

    #[test]
    fn macroscope_cancelled_body_reads_are_closed() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{Arc, Barrier, mpsc},
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let barrier = Arc::new(Barrier::new(4));
        let (started_tx, started_rx) = mpsc::channel();
        let (held_tx, held_rx) = mpsc::channel();
        let server_barrier = barrier.clone();
        let server = std::thread::spawn(move || {
            let mut handlers = Vec::new();
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().unwrap();
                let started_tx = started_tx.clone();
                let held_tx = held_tx.clone();
                let barrier = server_barrier.clone();
                handlers.push(std::thread::spawn(move || {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        socket.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                    }
                    socket
                        .write_all(
                            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 4\r\nContent-Range: bytes 0-3/4\r\n\r\nx",
                        )
                        .unwrap();
                    started_tx.send(()).unwrap();
                    barrier.wait();
                    socket
                        .set_read_timeout(Some(Duration::from_millis(200)))
                        .unwrap();
                    let mut byte = [0];
                    let held = socket.read(&mut byte).is_err_and(|error| {
                        matches!(
                            error.kind(),
                            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                        )
                    });
                    held_tx.send(held).unwrap();
                }));
            }
            for handler in handlers {
                handler.join().unwrap();
            }
        });
        let file = RemoteFile {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
                .unwrap(),
            url: url.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            position: 0,
            size: 0,
            start: 0,
            bytes: Arc::from([]),
            cache: None,
            max_size: 4,
        };
        for _ in 0..3 {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut worker_file = file.clone();
            worker_file.cancel = cancel.clone();
            let worker_url = url.clone();
            let worker =
                std::thread::spawn(move || worker_file.request_block_from(&worker_url, 0, 3, true));
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            cancel.store(true, Ordering::Relaxed);
            assert_eq!(
                worker.join().unwrap().unwrap_err().kind(),
                io::ErrorKind::Interrupted
            );
        }
        barrier.wait();
        let held = (0..3)
            .filter(|_| held_rx.recv_timeout(Duration::from_secs(1)).unwrap())
            .count();
        server.join().unwrap();
        assert_eq!(held, 0, "cancelled body readers remained active");
    }
}
