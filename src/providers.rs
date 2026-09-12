use serde_json::Value;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

pub struct Prepared {
    pub video: Arc<[u8]>,
    pub audio: Arc<[u8]>,
}
impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prepared")
            .field("video_bytes", &self.video.len())
            .field("audio_bytes", &self.audio.len())
            .finish()
    }
}

pub struct Resolved {
    pub video: String,
    pub audio: Option<String>,
    pub title: String,
    pub provider: String,
    pub prepared: Option<Prepared>,
    pub progressive: bool,
    pub fragmented: bool,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved")
            .field("provider", &self.provider)
            .field("has_video", &!self.video.is_empty())
            .field("has_audio", &self.audio.is_some())
            .field("prepared", &self.prepared)
            .field("progressive", &self.progressive)
            .field("fragmented", &self.fragmented)
            .finish_non_exhaustive()
    }
}

pub fn resolve(input: &str) -> Result<Resolved, String> {
    resolve_with_cancel(input, Arc::default())
}

pub fn resolve_with_cancel(input: &str, cancel: Arc<AtomicBool>) -> Result<Resolved, String> {
    if !input.contains("://") {
        return Ok(Resolved {
            video: input.into(),
            audio: None,
            title: std::path::Path::new(input)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            provider: "Local file".into(),
            prepared: None,
            progressive: false,
            fragmented: false,
        });
    }
    let url = reqwest::Url::parse(input).map_err(|_| "Invalid video URL")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
    {
        return Err("Use an HTTPS video URL".into());
    }
    match url.host_str().unwrap_or_default() {
        "youtube.com"
        | "www.youtube.com"
        | "m.youtube.com"
        | "youtu.be"
        | "www.youtube-nocookie.com" => youtube(
            &youtube_id(&url).ok_or("Invalid YouTube video link")?,
            cancel,
        ),
        "fixupx.com" | "www.fixupx.com" | "fxtwitter.com" | "www.fxtwitter.com" | "x.com"
        | "twitter.com" => fixtweet(&status_id(&url).ok_or("Invalid post link")?, &cancel),
        _ if crate::http::allowed(input) => Ok(Resolved {
            video: input.into(),
            audio: None,
            title: "Video".into(),
            provider: "Direct stream".into(),
            prepared: None,
            progressive: false,
            fragmented: false,
        }),
        _ => Err("This video host is not supported yet".into()),
    }
}

fn fetch(url: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let client = crate::request::Client::new()?;
    client.read(
        client.get(url),
        4 * 1024 * 1024,
        cancel,
        Instant::now() + Duration::from_secs(20),
    )
}

fn status_id(url: &reqwest::Url) -> Option<String> {
    let segments: Vec<_> = url.path_segments()?.collect();
    let index = segments.iter().position(|part| *part == "status")?;
    let id = *segments.get(index + 1)?;
    (id.len() <= 24 && !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| id.into())
}

fn fixtweet(id: &str, cancel: &AtomicBool) -> Result<Resolved, String> {
    let bytes = fetch(&format!("https://api.fxtwitter.com/status/{id}"), cancel)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid FixupX response")?;
    parse_fixtweet(&value)
}

fn parse_fixtweet(value: &Value) -> Result<Resolved, String> {
    let tweet = &value["tweet"];
    let videos = tweet["media"]["videos"]
        .as_array()
        .ok_or("This post has no playable video")?;
    let video = videos.first().ok_or("This post has no playable video")?;
    let url = video["variants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|variant| {
            variant["content_type"] == "video/mp4"
                && variant["url"].as_str().is_some_and(crate::http::allowed)
        })
        .max_by_key(|variant| variant["bitrate"].as_u64().unwrap_or(0))
        .and_then(|variant| variant["url"].as_str())
        .or_else(|| {
            video["url"]
                .as_str()
                .filter(|url| crate::http::allowed(url))
        })
        .ok_or("This post has no supported video stream")?;
    Ok(Resolved {
        video: url.into(),
        audio: None,
        title: tweet["author"]["name"]
            .as_str()
            .map(|author| format!("Video by {author}"))
            .unwrap_or_else(|| "Video".into()),
        provider: "FixupX".into(),
        prepared: None,
        progressive: false,
        fragmented: false,
    })
}

fn youtube_id(url: &reqwest::Url) -> Option<String> {
    let id = if url.host_str() == Some("youtu.be") {
        url.path_segments()?.next()?.to_owned()
    } else if url.path() == "/watch" {
        url.query_pairs()
            .find(|(key, _)| key == "v")?
            .1
            .into_owned()
    } else {
        let mut path = url.path_segments()?;
        if !matches!(path.next()?, "embed" | "shorts" | "live") {
            return None;
        }
        path.next()?.to_owned()
    };
    (id.len() == 11
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some(id)
}

fn youtube(id: &str, cancel: Arc<AtomicBool>) -> Result<Resolved, String> {
    const DESKTOP_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";
    let client = crate::request::Client::new()?;
    let page = client.read(
        client
            .get(&format!(
                "https://www.youtube.com/watch?v={id}&hl=en&bpctr=9999999999&has_verified=1"
            ))
            .header("User-Agent", DESKTOP_AGENT)
            .header("Cookie", "SOCS=CAI; PREF=hl=en&tz=UTC"),
        4 * 1024 * 1024,
        &cancel,
        Instant::now() + Duration::from_secs(20),
    )?;
    let text = std::str::from_utf8(&page).map_err(|_| "Invalid YouTube response")?;
    let player = [
        "var ytInitialPlayerResponse = ",
        "ytInitialPlayerResponse = ",
    ]
    .iter()
    .find_map(|marker| {
        let start = text.find(marker)? + marker.len();
        serde_json::Deserializer::from_str(&text[start..])
            .into_iter::<Value>()
            .next()?
            .ok()
    })
    .ok_or("YouTube did not expose playable stream data")?;
    match youtube_visionos(&client, text, id, &cancel) {
        Ok(resolved) => return Ok(resolved),
        Err(_) if cancel.load(std::sync::atomic::Ordering::Relaxed) => {
            return Err("Video loading cancelled".into());
        }
        Err(_) => {}
    }
    if player["playabilityStatus"]["status"] == "OK"
        && player["streamingData"]["serverAbrStreamingUrl"].is_string()
    {
        let prepared = crate::youtube::prepare(text, &player, cancel)?;
        return Ok(Resolved {
            video: String::new(),
            audio: None,
            title: player["videoDetails"]["title"]
                .as_str()
                .unwrap_or("YouTube video")
                .into(),
            provider: "YouTube".into(),
            prepared: Some(prepared),
            progressive: false,
            fragmented: false,
        });
    }
    parse_youtube(&player)
}

fn youtube_visionos(
    client: &crate::request::Client,
    page: &str,
    id: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<Resolved, String> {
    const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
    let visitor = json_after(page, "\"INNERTUBE_CONTEXT\":")
        .and_then(|context| context["client"]["visitorData"].as_str().map(str::to_owned))
        .ok_or("YouTube did not provide visitor data")?;
    if visitor.len() > 4096 {
        return Err("YouTube visitor data exceeds the limit".into());
    }
    let body = serde_json::json!({
        "context": {"client": {
            "clientName": "VISIONOS",
            "clientVersion": "1.02",
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "userAgent": USER_AGENT,
            "osName": "visionOS",
            "osVersion": "26.5.23O471",
            "hl": "en",
            "timeZone": "UTC",
            "utcOffsetMinutes": 0,
            "visitorData": visitor,
        }},
        "videoId": id,
        "playbackContext": {"contentPlaybackContext": {
            "html5Preference": "HTML5_PREF_WANTS",
        }},
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let endpoint =
        reqwest::Url::parse("https://www.youtube.com/youtubei/v1/player?prettyPrint=false")
            .map_err(|_| "Invalid YouTube player endpoint")?;
    let response = client.read(
        client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("X-YouTube-Client-Name", "101")
            .header("X-YouTube-Client-Version", "1.02")
            .header("X-Goog-Visitor-Id", &visitor)
            .header("Origin", "https://www.youtube.com")
            .header("User-Agent", USER_AGENT)
            .json(&body),
        4 * 1024 * 1024,
        cancel,
        Instant::now() + Duration::from_secs(20),
    )?;
    let player: Value =
        serde_json::from_slice(&response).map_err(|_| "Invalid YouTube player response")?;
    validate_youtube_video(&player)?;
    if let Some(resolved) = youtube_progressive(&player) {
        return Ok(resolved);
    }
    let mut resolved = parse_youtube(&player)?;
    resolved.progressive = true;
    resolved.fragmented = true;
    Ok(resolved)
}

fn youtube_progressive(player: &Value) -> Option<Resolved> {
    if player["playabilityStatus"]["status"] != "OK" {
        return None;
    }
    let video = player["streamingData"]["formats"]
        .as_array()?
        .iter()
        .filter(|format| {
            let mime = format["mimeType"].as_str().unwrap_or_default();
            format["url"].as_str().is_some_and(crate::http::allowed)
                && mime.starts_with("video/mp4")
                && mime.contains("avc1")
                && mime.contains("mp4a")
                && format["height"].as_u64().unwrap_or(0) <= 720
                && format["fps"].as_u64().unwrap_or(30) <= 30
        })
        .max_by_key(|format| {
            (
                format["height"].as_u64().unwrap_or(0),
                format["bitrate"].as_u64().unwrap_or(0),
            )
        })?;
    Some(Resolved {
        video: video["url"].as_str()?.into(),
        audio: None,
        title: player["videoDetails"]["title"]
            .as_str()
            .unwrap_or("YouTube video")
            .into(),
        provider: "YouTube".into(),
        prepared: None,
        progressive: true,
        fragmented: false,
    })
}

fn json_after(text: &str, marker: &str) -> Option<Value> {
    let start = text.find(marker)? + marker.len();
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()?
        .ok()
}

fn parse_youtube(player: &Value) -> Result<Resolved, String> {
    validate_youtube_video(player)?;
    if player["playabilityStatus"]["status"] != "OK" {
        return Err("YouTube requires sign-in or does not allow this playback".into());
    }
    let data = &player["streamingData"];
    let formats = data["formats"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(data["adaptiveFormats"].as_array().into_iter().flatten());
    let mut video: Option<&Value> = None;
    let mut audio: Option<&Value> = None;
    for format in formats {
        if !format["url"].as_str().is_some_and(crate::http::allowed) {
            continue;
        }
        let mime = format["mimeType"].as_str().unwrap_or_default();
        if is_h264_candidate(format) {
            if video.is_none_or(|previous| format["height"].as_u64() > previous["height"].as_u64())
            {
                video = Some(format);
            }
        } else if mime.starts_with("audio/mp4")
            && mime.contains("mp4a")
            && audio
                .is_none_or(|previous| format["bitrate"].as_u64() > previous["bitrate"].as_u64())
        {
            audio = Some(format);
        }
    }
    let video = video.ok_or_else(|| youtube_stream_error(data))?;
    let muxed = video["mimeType"]
        .as_str()
        .unwrap_or_default()
        .contains("mp4a");
    if !muxed && audio.is_none() {
        return Err("YouTube did not provide a supported audio stream".into());
    }
    Ok(Resolved {
        video: video["url"].as_str().unwrap().into(),
        audio: if muxed {
            None
        } else {
            audio
                .and_then(|value| value["url"].as_str())
                .map(str::to_owned)
        },
        title: player["videoDetails"]["title"]
            .as_str()
            .unwrap_or("YouTube video")
            .into(),
        provider: "YouTube".into(),
        prepared: None,
        progressive: false,
        fragmented: false,
    })
}

fn validate_youtube_video(player: &Value) -> Result<(), String> {
    let duration = player["videoDetails"]["lengthSeconds"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("YouTube did not provide a duration")?;
    if duration == 0 || duration > 1200 || player["videoDetails"]["isLiveContent"] == true {
        Err("YouTube clips must be recorded videos under 20 minutes".into())
    } else {
        Ok(())
    }
}

fn is_h264_candidate(format: &Value) -> bool {
    let mime = format["mimeType"].as_str().unwrap_or_default();
    mime.starts_with("video/mp4")
        && mime.contains("avc1")
        && format["height"].as_u64().unwrap_or(0) <= 720
        && format["fps"].as_u64().unwrap_or(30) <= 30
}

fn youtube_stream_error(data: &Value) -> &'static str {
    let formats = ["formats", "adaptiveFormats"]
        .iter()
        .flat_map(|key| data[key].as_array().into_iter().flatten());
    let mut direct = false;
    let mut cipher = false;
    for format in formats {
        if !is_h264_candidate(format) {
            continue;
        }
        direct |= format["url"].is_string();
        cipher |= format["signatureCipher"].is_string() || format["cipher"].is_string();
    }
    if direct {
        "YouTube did not provide a supported H.264 stream"
    } else if cipher {
        "This YouTube stream needs URL signature resolution, which is not supported yet"
    } else if data["serverAbrStreamingUrl"].is_string() {
        "This YouTube video uses SABR streaming, which is not supported yet"
    } else {
        "YouTube did not provide a supported media stream"
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_output_omits_signed_urls_and_titles() {
        let resolved = super::Resolved {
            video: "https://media.example/video?signature=private-video".into(),
            audio: Some("https://media.example/audio?signature=private-audio".into()),
            title: "private-title".into(),
            provider: "YouTube".into(),
            prepared: None,
            progressive: true,
            fragmented: true,
        };
        let output = format!("{resolved:?}");
        assert!(output.contains("YouTube"));
        assert!(!output.contains("private-"));
        assert!(!output.contains("https://"));
    }

    #[test]
    fn macroscope_ciphered_h264_reports_signature_resolution() {
        let player = serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {"lengthSeconds": "60", "isLiveContent": false},
            "streamingData": {
                "adaptiveFormats": [
                    {
                        "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                        "url": "https://rr1.googlevideo.com/audio"
                    },
                    {
                        "mimeType": "video/webm; codecs=\"vp9\"",
                        "url": "https://rr1.googlevideo.com/video"
                    },
                    {
                        "mimeType": "video/mp4; codecs=\"avc1.4d401f\"",
                        "height": 720,
                        "fps": 30,
                        "signatureCipher": "s=encrypted"
                    }
                ]
            }
        });
        assert_eq!(
            parse_youtube(&player).unwrap_err(),
            "This YouTube stream needs URL signature resolution, which is not supported yet"
        );
    }

    #[test]
    fn macroscope_desktop_fallback_rejects_long_video() {
        let mut player = serde_json::json!({
            "playabilityStatus": {"status": "OK"},
            "videoDetails": {"lengthSeconds": "1201", "isLiveContent": false},
            "streamingData": {"formats": [{
                "mimeType": "video/mp4; codecs=\"avc1.4d401f, mp4a.40.2\"",
                "height": 720,
                "fps": 30,
                "url": "https://rr1.googlevideo.com/video"
            }]}
        });
        assert_eq!(
            parse_youtube(&player).unwrap_err(),
            "YouTube clips must be recorded videos under 20 minutes"
        );
        player["videoDetails"]["lengthSeconds"] = "60".into();
        player["videoDetails"]["isLiveContent"] = true.into();
        assert_eq!(
            parse_youtube(&player).unwrap_err(),
            "YouTube clips must be recorded videos under 20 minutes"
        );
    }
    fn stalled_request_cancels(body_started: bool) {
        use std::{
            io::{Read, Write},
            net::TcpListener,
            sync::{atomic::Ordering, mpsc},
            time::Instant,
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
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
            if body_started {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                    .unwrap();
            }
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (result_tx, result_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            result_tx.send(fetch(&url, &worker_cancel)).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let began = Instant::now();
        cancel.store(true, Ordering::Relaxed);
        let result = result_rx.recv_timeout(Duration::from_millis(500));
        let elapsed = began.elapsed();
        release_tx.send(()).unwrap();
        server.join().unwrap();
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Err("Video loading cancelled".into()));
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn macroscope_stalled_provider_headers_cancel_promptly() {
        stalled_request_cancels(false);
    }

    #[test]
    fn macroscope_stalled_provider_body_cancels_promptly() {
        stalled_request_cancels(true);
    }

    use super::*;
    #[test]
    fn providers_reject_unsafe_addresses() {
        for input in [
            "http://youtube.com/watch?v=abcdefghijk",
            "https://user@youtube.com/watch?v=abcdefghijk",
            "https://youtube.com.evil.test/watch?v=abcdefghijk",
            "file:///secret",
            "https://127.0.0.1/video.mp4",
        ] {
            assert!(resolve(input).is_err());
        }
    }
    #[test]
    fn video_ids_are_parsed_without_forwarding_queries() {
        for link in [
            "https://youtu.be/abcdefghijk?t=2",
            "https://www.youtube.com/watch?v=abcdefghijk&list=private",
            "https://www.youtube.com/embed/abcdefghijk",
        ] {
            assert_eq!(
                youtube_id(&reqwest::Url::parse(link).unwrap()).as_deref(),
                Some("abcdefghijk")
            );
        }
        assert!(
            youtube_id(&reqwest::Url::parse("https://youtube.com/watch?v=../bad").unwrap())
                .is_none()
        );
    }
    #[test]
    fn metadata_cannot_redirect_media_to_private_hosts() {
        let value =
            serde_json::json!({"tweet":{"media":{"videos":[{"url":"https://127.0.0.1/secret"}]}}});
        assert!(parse_fixtweet(&value).is_err());
        let player = serde_json::json!({"playabilityStatus":{"status":"OK"},"videoDetails":{"lengthSeconds":"60","isLiveContent":false},"streamingData":{"formats":[{"url":"https://evil.test/video","mimeType":"video/mp4; codecs=avc1,mp4a"}]}});
        assert!(parse_youtube(&player).is_err());
    }
    #[test]
    fn youtube_selects_muxed_h264_or_separate_aac() {
        let player = serde_json::json!({"playabilityStatus":{"status":"OK"},"videoDetails":{"lengthSeconds":"60","isLiveContent":false},"streamingData":{"formats":[{"url":"https://rr1.googlevideo.com/video","mimeType":"video/mp4; codecs=avc1,mp4a","height":360}]}});
        assert!(parse_youtube(&player).unwrap().audio.is_none());
        assert!(youtube_progressive(&player).unwrap().progressive);
    }
}
