use serde_json::Value;
use std::{io::Read, time::Duration};

#[derive(Debug)]
pub struct Resolved {
    pub video: String,
    pub audio: Option<String>,
    pub title: String,
    pub provider: String,
}

pub fn resolve(input: &str) -> Result<Resolved, String> {
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
        | "www.youtube-nocookie.com" => {
            youtube(&youtube_id(&url).ok_or("Invalid YouTube video link")?)
        }
        "fixupx.com" | "www.fixupx.com" | "fxtwitter.com" | "www.fxtwitter.com" | "x.com"
        | "twitter.com" => fixtweet(&status_id(&url).ok_or("Invalid post link")?),
        _ if crate::http::allowed(input) => Ok(Resolved {
            video: input.into(),
            audio: None,
            title: "Video".into(),
            provider: "Direct stream".into(),
        }),
        _ => Err("This video host is not supported yet".into()),
    }
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent("rust-media/0.1")
        .build()
        .map_err(|_| "Could not start provider request")?;
    let response = client
        .get(url)
        .send()
        .map_err(|_| "Could not reach the video provider")?;
    if response.status().is_redirection() {
        return Err("The provider requires a redirect or sign-in".into());
    }
    let response = response
        .error_for_status()
        .map_err(|_| "The provider refused the request")?;
    let mut bytes = Vec::new();
    response
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Provider response was interrupted")?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Provider response exceeds the size limit".into());
    }
    Ok(bytes)
}

fn status_id(url: &reqwest::Url) -> Option<String> {
    let segments: Vec<_> = url.path_segments()?.collect();
    let index = segments.iter().position(|part| *part == "status")?;
    let id = *segments.get(index + 1)?;
    (id.len() <= 24 && !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| id.into())
}

fn fixtweet(id: &str) -> Result<Resolved, String> {
    let bytes = fetch(&format!("https://api.fxtwitter.com/status/{id}"))?;
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

fn youtube(id: &str) -> Result<Resolved, String> {
    let page = fetch(&format!("https://www.youtube.com/watch?v={id}&hl=en"))?;
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
    parse_youtube(&player)
}

fn parse_youtube(player: &Value) -> Result<Resolved, String> {
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
        if mime.starts_with("video/mp4")
            && mime.contains("avc1")
            && format["height"].as_u64().unwrap_or(0) <= 720
            && format["fps"].as_u64().unwrap_or(30) <= 30
        {
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
    })
}

fn youtube_stream_error(data: &Value) -> &'static str {
    let formats = ["formats", "adaptiveFormats"]
        .iter()
        .flat_map(|key| data[key].as_array().into_iter().flatten());
    let mut direct = false;
    let mut cipher = false;
    for format in formats {
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
        let player = serde_json::json!({"playabilityStatus":{"status":"OK"},"streamingData":{"formats":[{"url":"https://evil.test/video","mimeType":"video/mp4; codecs=avc1,mp4a"}]}});
        assert!(parse_youtube(&player).is_err());
    }
    #[test]
    fn youtube_selects_muxed_h264_or_separate_aac() {
        let player = serde_json::json!({"playabilityStatus":{"status":"OK"},"streamingData":{"formats":[{"url":"https://rr1.googlevideo.com/video","mimeType":"video/mp4; codecs=avc1,mp4a","height":360}]}});
        assert!(parse_youtube(&player).unwrap().audio.is_none());
    }
}
