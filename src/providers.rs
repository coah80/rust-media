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
    pub start_time: f64,
    pub end_time: Option<f64>,
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
            start_time: 0.,
            end_time: None,
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
        | "www.youtube-nocookie.com" => resolve_youtube_link(&url, cancel),
        "fixupx.com" | "www.fixupx.com" | "fxtwitter.com" | "www.fxtwitter.com" | "x.com"
        | "twitter.com" => fixtweet(&status_id(&url).ok_or("Invalid post link")?, &cancel),
        "streamable.com" | "www.streamable.com" => streamable(
            streamable_id(&url).ok_or("Invalid Streamable video link")?,
            &cancel,
        ),
        "imgur.com" | "www.imgur.com" | "i.imgur.com" => imgur(&url),
        "giphy.com" | "www.giphy.com" => giphy(&url),
        _ if crate::http::public_url(input) => Ok(Resolved {
            start_time: 0.,
            end_time: None,
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

fn native_link(video: String, provider: &str) -> Resolved {
    Resolved {
        start_time: 0.,
        end_time: None,
        video,
        audio: None,
        title: "Video".into(),
        provider: provider.into(),
        prepared: None,
        progressive: false,
        fragmented: false,
    }
}

fn imgur(url: &reqwest::Url) -> Result<Resolved, String> {
    let path = url.path().trim_start_matches('/');
    let id = path
        .strip_suffix(".gifv")
        .or_else(|| path.strip_suffix(".mp4"))
        .unwrap_or(path);
    if !(5..=12).contains(&id.len()) || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Use an individual Imgur video or GIFV link".into());
    }
    Ok(native_link(
        format!("https://i.imgur.com/{id}.mp4"),
        "Imgur",
    ))
}

fn giphy(url: &reqwest::Url) -> Result<Resolved, String> {
    let path = url
        .path()
        .strip_prefix("/gifs/")
        .or_else(|| url.path().strip_prefix("/embed/"))
        .ok_or("Use a GIPHY GIF or embed link")?;
    if path.contains('/') {
        return Err("Invalid GIPHY link".into());
    }
    let id = path.rsplit('-').next().unwrap_or_default();
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("Invalid GIPHY link".into());
    }
    Ok(native_link(
        format!("https://media.giphy.com/media/{id}/giphy.mp4"),
        "GIPHY",
    ))
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

fn streamable_id(url: &reqwest::Url) -> Option<&str> {
    let path = url.path().strip_prefix('/')?;
    let id = path
        .strip_prefix("e/")
        .or_else(|| path.strip_prefix("o/"))
        .unwrap_or(path);
    let id = id.strip_suffix('/').unwrap_or(id);
    (!id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_alphanumeric()))
        .then_some(id)
}

fn streamable(id: &str, cancel: &AtomicBool) -> Result<Resolved, String> {
    let bytes = fetch(&format!("https://api.streamable.com/videos/{id}"), cancel)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid Streamable response")?;
    parse_streamable(&value)
}

fn parse_streamable(value: &Value) -> Result<Resolved, String> {
    if value["status"].as_u64() != Some(2) {
        return Err("This Streamable video is unavailable or still processing".into());
    }
    let files = value["files"]
        .as_object()
        .ok_or("Invalid Streamable response")?;
    let video = files
        .iter()
        .filter_map(|(format, file)| {
            if !(format == "mp4" || format.starts_with("mp4-"))
                || file["status"].as_u64() != Some(2)
            {
                return None;
            }
            let width = file["width"].as_u64()?;
            let height = file["height"].as_u64()?;
            if width == 0
                || height == 0
                || width > 1920
                || height > 1920
                || file["size"]
                    .as_u64()
                    .is_some_and(|size| size > 2 * 1024 * 1024 * 1024)
            {
                return None;
            }
            let raw = file["url"].as_str()?;
            let url = if raw.starts_with("//") {
                format!("https:{raw}")
            } else {
                raw.into()
            };
            let parsed = reqwest::Url::parse(&url).ok()?;
            if !crate::http::allowed(&url)
                || !parsed.host_str().is_some_and(crate::http::streamable_cdn)
                || !parsed.path().ends_with(".mp4")
            {
                return None;
            }
            let fits_output = width.max(height) <= 1280 && width.min(height) <= 720;
            Some((
                (
                    fits_output,
                    width * height,
                    file["bitrate"].as_u64().unwrap_or(0),
                ),
                url,
            ))
        })
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, url)| url)
        .ok_or("Streamable did not provide a supported MP4 stream")?;
    Ok(Resolved {
        video,
        start_time: 0.,
        end_time: None,
        audio: None,
        title: value["title"].as_str().unwrap_or("Video").into(),
        provider: "Streamable".into(),
        prepared: None,
        progressive: false,
        fragmented: false,
    })
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
        start_time: 0.,
        end_time: None,
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

fn link_time(url: &reqwest::Url) -> f64 {
    let query = url
        .query_pairs()
        .find(|(key, _)| key == "t")
        .or_else(|| url.query_pairs().find(|(key, _)| key == "start"))
        .map(|(_, value)| value.into_owned());
    let value = query
        .as_deref()
        .or_else(|| url.fragment().and_then(|f| f.strip_prefix("t=")));
    value.and_then(parse_time).unwrap_or(0.)
}

fn parse_time(value: &str) -> Option<f64> {
    if value.is_empty() || value.len() > 32 {
        return None;
    }
    if value.bytes().all(|b| b.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .filter(|v| *v <= 604800)
            .map(|v| v as f64);
    }
    let mut total = 0_u64;
    let mut number = 0_u64;
    let mut digits = false;
    let mut previous = 3601;
    for byte in value.bytes() {
        if byte.is_ascii_digit() {
            number = number
                .checked_mul(10)?
                .checked_add(u64::from(byte - b'0'))?;
            digits = true;
        } else {
            let multiplier = match byte {
                b'h' => 3600,
                b'm' => 60,
                b's' => 1,
                _ => return None,
            };
            if !digits || multiplier >= previous {
                return None;
            }
            total = total.checked_add(number.checked_mul(multiplier)?)?;
            previous = multiplier;
            number = 0;
            digits = false;
        }
    }
    (!digits && total <= 604800).then_some(total as f64)
}

fn resolve_youtube_link(url: &reqwest::Url, cancel: Arc<AtomicBool>) -> Result<Resolved, String> {
    if let Some(clip) = url.path().strip_prefix("/clip/") {
        if clip.is_empty()
            || clip.len() > 128
            || !clip
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("Invalid YouTube clip link".into());
        }
        let bytes = fetch(&format!("https://www.youtube.com/clip/{clip}"), &cancel)?;
        let page = std::str::from_utf8(&bytes).map_err(|_| "Invalid YouTube clip response")?;
        let (id, start, end) = parse_youtube_clip(page, clip)?;
        let mut resolved = youtube(&id, cancel)?;
        resolved.start_time = start;
        resolved.end_time = Some(end);
        return Ok(resolved);
    }
    let mut resolved = youtube(
        &youtube_id(url).ok_or("Invalid YouTube video link")?,
        cancel,
    )?;
    resolved.start_time = link_time(url);
    Ok(resolved)
}

fn parse_youtube_clip(page: &str, clip: &str) -> Result<(String, f64, f64), String> {
    let player = json_after(page, "ytInitialPlayerResponse = ")
        .ok_or("YouTube did not expose clip metadata")?;
    let config = &player["clipConfig"];
    if config["postId"].as_str() != Some(clip) {
        return Err("YouTube did not return the requested clip".into());
    }
    let id = player["videoDetails"]["videoId"]
        .as_str()
        .filter(|id| {
            id.len() == 11
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or("YouTube did not expose the clip's source video")?;
    let start = config["startTimeMs"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok());
    let end = config["endTimeMs"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok());
    match (start, end) {
        (Some(start), Some(end)) if start < end && end <= 604800000 && end - start <= 60000 => {
            Ok((id.into(), start as f64 / 1000., end as f64 / 1000.))
        }
        _ => Err("YouTube returned invalid clip times".into()),
    }
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
            start_time: 0.,
            end_time: None,
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
    resolved.progressive = youtube_prefetch(&player);
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
        start_time: 0.,
        end_time: None,
        audio: None,
        title: player["videoDetails"]["title"]
            .as_str()
            .unwrap_or("YouTube video")
            .into(),
        provider: "YouTube".into(),
        prepared: None,
        progressive: youtube_prefetch(player),
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
        start_time: 0.,
        end_time: None,
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

fn youtube_prefetch(player: &Value) -> bool {
    player["videoDetails"]["lengthSeconds"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|duration| duration <= 1200)
}

fn validate_youtube_video(player: &Value) -> Result<(), String> {
    let duration = player["videoDetails"]["lengthSeconds"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or("YouTube did not provide a duration")?;
    if duration == 0 || duration > 604800 || player["videoDetails"]["isLiveContent"] == true {
        Err("Choose a recorded YouTube video under seven days".into())
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
    fn additional_embed_links_keep_media_on_fixed_hosts() {
        for (input, expected) in [
            (
                "https://imgur.com/owLfF25",
                "https://i.imgur.com/owLfF25.mp4",
            ),
            (
                "https://i.imgur.com/owLfF25.gifv",
                "https://i.imgur.com/owLfF25.mp4",
            ),
            (
                "https://giphy.com/gifs/example-cZ7rmKfFYOvYI",
                "https://media.giphy.com/media/cZ7rmKfFYOvYI/giphy.mp4",
            ),
            (
                "https://giphy.com/embed/cZ7rmKfFYOvYI",
                "https://media.giphy.com/media/cZ7rmKfFYOvYI/giphy.mp4",
            ),
        ] {
            assert_eq!(super::resolve(input).unwrap().video, expected);
        }
        for input in [
            "https://imgur.com/a/owLfF25",
            "https://imgur.com/gallery/owLfF25",
            "https://giphy.com/gifs/abc/def",
            "https://giphy.com/gifs/%2fexample",
            "https://user@i.imgur.com/owLfF25.mp4",
            "https://media.giphy.com.evil.test/media/abc/giphy.mp4",
        ] {
            assert!(super::resolve(input).is_err(), "accepted {input}");
        }
    }
    #[test]
    fn timestamps_are_bounded_and_preserved() {
        for (suffix, expected) in [
            ("?t=1479", 1479.),
            ("?si=ignored&t=24m39s", 1479.),
            ("?start=12", 12.),
            ("#t=1h2m3s", 3723.),
            ("?t=NaN", 0.),
            ("?t=-1", 0.),
            ("?t=99999999999999999999999", 0.),
        ] {
            let url = format!("https://youtu.be/4YJKc8PVGio{suffix}")
                .parse()
                .unwrap();
            assert_eq!(super::link_time(&url), expected);
        }
        for value in ["1s2h", "1h1h", "1.5", "1m2", "s", "", "604801"] {
            assert!(super::parse_time(value).is_none());
        }
    }

    #[test]
    fn clip_metadata_must_match_id_and_valid_time_window() {
        let mut player = serde_json::json!({
            "videoDetails": {"videoId": "4YJKc8PVGio"},
            "clipConfig": {"postId": "clip-fixture", "startTimeMs": "1449933", "endTimeMs": "1462563"}
        });
        let parse = |value: &serde_json::Value| {
            super::parse_youtube_clip(
                &format!("var ytInitialPlayerResponse = {value};"),
                "clip-fixture",
            )
        };
        assert_eq!(
            parse(&player).unwrap(),
            ("4YJKc8PVGio".into(), 1449.933, 1462.563)
        );
        player["clipConfig"]["postId"] = "different-clip".into();
        assert!(parse(&player).is_err());
        player["clipConfig"]["postId"] = "clip-fixture".into();
        for end in ["1449933", "0", "9999999999999999999999", "1600000"] {
            player["clipConfig"]["endTimeMs"] = end.into();
            assert!(parse(&player).is_err());
        }
        player["clipConfig"]["endTimeMs"] = "1462563".into();
        player["videoDetails"]["videoId"] = "../../local".into();
        assert!(parse(&player).is_err());
    }

    #[test]
    fn streamable_links_reject_extra_paths_and_encoded_ids() {
        for path in ["/abc123", "/e/abc123", "/o/abc123/", "/abc123?src=player"] {
            let url = format!("https://streamable.com{path}").parse().unwrap();
            assert_eq!(super::streamable_id(&url), Some("abc123"));
        }
        for path in [
            "/",
            "/e/",
            "/abc/other",
            "/%61bc",
            "/abc.mp4",
            "/e/abc/other",
        ] {
            let url = format!("https://streamable.com{path}").parse().unwrap();
            assert_eq!(super::streamable_id(&url), None);
        }
    }

    fn streamable_fixture() -> serde_json::Value {
        serde_json::json!({"status": 2, "title": "Fixture", "files": {
            "mp4": {"status": 2, "url": "//cdn-cf-east.streamable.com/video/mp4/abc.mp4?signature=fixture", "width": 1280, "height": 720},
            "mp4-mobile": {"status": 2, "url": "https://cdn-b-east.streamable.com/video/mp4-mobile/abc.mp4", "width": 640, "height": 360}
        }})
    }

    #[test]
    fn streamable_metadata_cannot_redirect_to_untrusted_media() {
        for url in [
            "https://127.0.0.1/abc.mp4",
            "https://cdn-cf-east.streamable.com.evil.test/abc.mp4",
            "https://evil.cdn-cf-east.streamable.com/abc.mp4",
            "https://streamable.com/abc.mp4",
            "https://user@cdn-cf-east.streamable.com/abc.mp4",
            "https://cdn-cf-east.streamable.com:444/abc.mp4",
            "http://cdn-cf-east.streamable.com/abc.mp4",
            "https://video.twimg.com/abc.mp4",
            "https://cdn-cf-east.streamable.com/abc.m3u8",
        ] {
            let mut value = streamable_fixture();
            value["files"].as_object_mut().unwrap().remove("mp4-mobile");
            value["files"]["mp4"]["url"] = url.into();
            assert!(super::parse_streamable(&value).is_err(), "accepted {url}");
        }
        let resolved = super::parse_streamable(&streamable_fixture()).unwrap();
        assert!(
            resolved
                .video
                .starts_with("https://cdn-cf-east.streamable.com/")
        );
        assert!(!resolved.progressive);
        assert!(resolved.prepared.is_none());
    }

    #[test]
    fn streamable_rejects_oversized_and_unready_variants() {
        let mut value = streamable_fixture();
        value["files"]["mp4"]["width"] = 3840.into();
        assert!(
            super::parse_streamable(&value)
                .unwrap()
                .video
                .contains("mp4-mobile")
        );
        value["files"]["mp4-mobile"]["size"] = (2_u64 * 1024 * 1024 * 1024 + 1).into();
        assert!(super::parse_streamable(&value).is_err());
        value = streamable_fixture();
        value["status"] = 1.into();
        assert!(super::parse_streamable(&value).is_err());
        value["status"] = 2.into();
        value["files"]["mp4"]["status"] = 1.into();
        value["files"]["mp4-mobile"]["status"] = 1.into();
        assert!(super::parse_streamable(&value).is_err());
    }

    #[test]
    fn debug_output_omits_signed_urls_and_titles() {
        let resolved = super::Resolved {
            start_time: 0.,
            end_time: None,
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
    fn long_youtube_videos_use_ranges_and_live_videos_are_rejected() {
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
        assert!(!parse_youtube(&player).unwrap().progressive);
        assert!(!super::youtube_progressive(&player).unwrap().progressive);
        player["videoDetails"]["lengthSeconds"] = "60".into();
        player["videoDetails"]["isLiveContent"] = true.into();
        assert_eq!(
            parse_youtube(&player).unwrap_err(),
            "Choose a recorded YouTube video under seven days"
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
}
