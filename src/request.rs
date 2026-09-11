use std::{
    future::Future,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

static RUNTIME: OnceLock<Result<tokio::runtime::Runtime, String>> = OnceLock::new();

pub(crate) fn run<T: Send>(future: impl Future<Output = T> + Send) -> Result<T, String> {
    let runtime = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .max_blocking_threads(2)
                .enable_all()
                .build()
                .map_err(|_| "Could not start video networking".into())
        })
        .as_ref()
        .map_err(Clone::clone)?;
    if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|scope| {
            scope
                .spawn(move || runtime.block_on(future))
                .join()
                .map_err(|_| "Video networking failed".into())
        })
    } else {
        Ok(runtime.block_on(future))
    }
}

pub(crate) struct Client(reqwest::Client);

impl Client {
    pub fn new() -> Result<Self, String> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .user_agent("rust-media/0.1")
            .build()
            .map(Self)
            .map_err(|_| "Could not start the video request".into())
    }

    pub fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.0.get(url)
    }

    pub fn post(&self, url: reqwest::Url) -> reqwest::RequestBuilder {
        self.0.post(url)
    }

    pub fn read(
        &self,
        request: reqwest::RequestBuilder,
        max: usize,
        cancel: &AtomicBool,
        deadline: Instant,
    ) -> Result<Vec<u8>, String> {
        run(async move {
            let interrupted = async {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break "Video loading cancelled";
                    }
                    if Instant::now() >= deadline {
                        break "Video loading timed out";
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            let receive = async {
                let mut response = request
                    .send()
                    .await
                    .map_err(|_| "Could not reach the video provider")?;
                if !response.status().is_success() {
                    return Err(format!(
                        "Video provider refused the request, HTTP {}",
                        response.status().as_u16()
                    ));
                }
                if response
                    .content_length()
                    .is_some_and(|size| size > max as u64)
                {
                    return Err("Video response exceeds the size limit".into());
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|_| "Video response was interrupted")?
                {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Video loading cancelled".into());
                    }
                    if Instant::now() >= deadline {
                        return Err("Video loading timed out".into());
                    }
                    if chunk.len() > max - bytes.len() {
                        return Err("Video response exceeds the size limit".into());
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(bytes)
            };
            tokio::select! {
                biased;
                reason = interrupted => Err(reason.into()),
                result = receive => result,
            }
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };

    fn response(bytes: &'static [u8], max: usize) -> Result<Vec<u8>, String> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
            }
            let _ = socket.write_all(bytes);
        });
        let client = Client::new().unwrap();
        let result = client.read(
            client.post(url.parse().unwrap()),
            max,
            &AtomicBool::new(false),
            Instant::now() + Duration::from_secs(2),
        );
        server.join().unwrap();
        result
    }

    #[test]
    fn body_limits_and_truncation_are_enforced() {
        assert_eq!(
            response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc", 3).unwrap(),
            b"abc"
        );
        assert_eq!(
            response(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd", 3).unwrap_err(),
            "Video response exceeds the size limit"
        );
        assert_eq!(
            response(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n0\r\n\r\n",
                3
            )
            .unwrap_err(),
            "Video response exceeds the size limit"
        );
        assert!(response(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabc", 4).is_err());
    }

    #[test]
    fn redirects_and_rate_limits_do_not_trigger_another_request() {
        assert_eq!(response(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\n\r\n", 3).unwrap_err(), "Video provider refused the request, HTTP 302");
        assert_eq!(
            response(
                b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 60\r\nContent-Length: 0\r\n\r\n",
                3
            )
            .unwrap_err(),
            "Video provider refused the request, HTTP 429"
        );
    }

    #[test]
    fn cancellation_and_deadline_prevent_starting_a_request() {
        let client = Client::new().unwrap();
        let request = || client.get("http://127.0.0.1:1");
        assert_eq!(
            client
                .read(
                    request(),
                    1,
                    &AtomicBool::new(true),
                    Instant::now() + Duration::from_secs(1)
                )
                .unwrap_err(),
            "Video loading cancelled"
        );
        assert_eq!(
            client
                .read(request(), 1, &AtomicBool::new(false), Instant::now())
                .unwrap_err(),
            "Video loading timed out"
        );
    }

    #[test]
    fn macroscope_request_is_safe_inside_tokio_runtime() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            assert_eq!(
                response(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc", 3).unwrap(),
                b"abc"
            );
        });
    }
}
