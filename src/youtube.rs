use crate::{http, providers::Prepared, sabr_proto as p};
use base64::Engine;
use prost::Message;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{self, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_MEDIA: usize = 128 * 1024 * 1024;
const MAX_PART: usize = 8 * 1024 * 1024;

struct Track {
    id: p::FormatId,
    initialized: bool,
    init: Vec<u8>,
    segments: BTreeMap<i32, (i64, i64, Vec<u8>)>,
    end: i64,
    last: i32,
    final_segment: Option<i64>,
}
impl Track {
    fn new(format: &Value) -> Result<Self, String> {
        Ok(Self {
            id: p::FormatId {
                itag: Some(
                    format["itag"]
                        .as_u64()
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or("Missing YouTube format ID")?,
                ),
                last_modified: Some(
                    format["lastModified"]
                        .as_str()
                        .and_then(|v| v.parse().ok())
                        .ok_or("Missing YouTube format revision")?,
                ),
                xtags: format["xtags"].as_str().map(str::to_owned),
            },
            initialized: false,
            init: Vec::new(),
            segments: BTreeMap::new(),
            end: 0,
            last: 0,
            final_segment: None,
        })
    }
    fn complete(&self) -> bool {
        !self.init.is_empty()
            && self
                .final_segment
                .is_some_and(|end| i64::from(self.last) >= end)
    }
    fn range(&self) -> Option<p::Range> {
        (self.last > 0).then(|| p::Range {
            format: Some(self.id.clone()),
            start: Some(0),
            duration: Some(self.end),
            first: Some(1),
            last: Some(self.last),
        })
    }
    fn record(&mut self, header: &p::Header, bytes: Vec<u8>) -> Result<(), String> {
        if header.init == Some(true) {
            if self.init.is_empty() {
                self.init = bytes;
            } else if self.init != bytes {
                return Err("YouTube changed the stream initialization".into());
            }
        } else {
            let sequence = header.sequence.ok_or("Missing segment sequence")?;
            if sequence <= 0 || sequence > 100_000 {
                return Err("Invalid YouTube segment sequence".into());
            }
            if let std::collections::btree_map::Entry::Vacant(entry) = self.segments.entry(sequence)
            {
                let (start, duration) = segment_time(header)?;
                if start < 0 || duration <= 0 {
                    return Err("YouTube returned discontinuous media".into());
                }
                entry.insert((start, duration, bytes));
                while let Some((start, duration, _)) = self.segments.get(&(self.last + 1)) {
                    if start.abs_diff(self.end) > 2 {
                        return Err("YouTube returned discontinuous media".into());
                    }
                    self.end = start.checked_add(*duration).ok_or("Invalid media time")?;
                    self.last += 1;
                }
            }
        }
        Ok(())
    }
    fn finish(self, cancel: &AtomicBool, remux: bool) -> Result<Arc<[u8]>, String> {
        let mut bytes = self.init;
        for (_, _, segment) in self.segments.into_values() {
            bytes.extend(segment);
        }
        if remux {
            crate::fragment::remux(&bytes, cancel).map(Arc::from)
        } else {
            Ok(Arc::from(bytes))
        }
    }
}

pub(crate) fn prepare(
    page: &str,
    player: &Value,
    cancel: Arc<AtomicBool>,
) -> Result<Prepared, String> {
    let duration = player["videoDetails"]["lengthSeconds"]
        .as_str()
        .and_then(|v| v.parse::<u64>().ok())
        .ok_or("YouTube did not provide a duration")?;
    if duration == 0 || duration > 1200 || player["videoDetails"]["isLiveContent"] == true {
        return Err("YouTube clips must be recorded videos under 20 minutes".into());
    }
    let formats = player["streamingData"]["adaptiveFormats"]
        .as_array()
        .ok_or("YouTube did not provide media formats")?;
    let video = formats
        .iter()
        .filter(|f| {
            f["mimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("video/mp4") && m.contains("avc1"))
                && f["height"].as_u64().is_some_and(|h| h <= 720)
                && f["fps"].as_u64().unwrap_or(30) <= 30
        })
        .max_by_key(|f| f["height"].as_u64())
        .ok_or("YouTube did not provide supported H.264 video")?;
    let audio = formats
        .iter()
        .filter(|f| {
            f["mimeType"]
                .as_str()
                .is_some_and(|m| m.starts_with("audio/mp4") && m.contains("mp4a"))
        })
        .min_by_key(|f| f["bitrate"].as_u64())
        .ok_or("YouTube did not provide supported AAC audio")?;
    let mut tracks = [Track::new(video)?, Track::new(audio)?];
    let context = json_after(page, "\"INNERTUBE_CONTEXT\":")
        .ok_or("YouTube did not provide client metadata")?;
    let version = context["client"]["clientVersion"]
        .as_str()
        .ok_or("YouTube did not provide a client version")?;
    if version.len() > 128 {
        return Err("Invalid YouTube client version".into());
    }
    let config = player["playerConfig"]["mediaCommonConfig"]["mediaUstreamerRequestConfig"]["videoPlaybackUstreamerConfig"].as_str().ok_or("YouTube did not provide streaming configuration")?;
    let config = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(config.trim_end_matches('='))
        .map_err(|_| "Invalid YouTube streaming configuration")?;
    if config.len() > 65536 {
        return Err("YouTube configuration exceeds the limit".into());
    }
    let mut url = reqwest::Url::parse(
        player["streamingData"]["serverAbrStreamingUrl"]
            .as_str()
            .ok_or("Missing YouTube stream URL")?,
    )
    .map_err(|_| "Invalid YouTube stream URL")?;
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(20))
        .user_agent("rust-media/0.1")
        .build()
        .map_err(|_| "Could not start the YouTube transport")?;
    if let Some(input) = url
        .query_pairs()
        .find(|(key, _)| key == "n")
        .map(|(_, value)| value.into_owned())
    {
        if input.len() > 4096 {
            return Err("YouTube stream parameter exceeds the limit".into());
        }
        let asset = json_after(page, "\"jsUrl\":").ok_or("Missing YouTube player script")?;
        let asset = asset.as_str().ok_or("Invalid YouTube player script")?;
        if !asset.starts_with("/s/player/")
            || asset.len() > 256
            || !asset
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/_.-".contains(&byte))
            || asset.contains("..")
            || asset.contains('?')
            || asset.contains('#')
        {
            return Err("Invalid YouTube player script path".into());
        }
        let response = client
            .get(format!("https://www.youtube.com{asset}"))
            .send()
            .map_err(|_| "Could not load the YouTube player script")?;
        if !response.status().is_success() {
            return Err("YouTube refused its player script".into());
        }
        let code = read_bounded(response, 8 * 1024 * 1024, &cancel)?;
        let code = String::from_utf8(code).map_err(|_| "Invalid YouTube player script")?;
        let output = crate::script::transform(code, input, cancel.clone())?;
        let pairs: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.query_pairs_mut().clear().extend_pairs(
            pairs
                .iter()
                .map(|(k, v)| (k.as_str(), if k == "n" { &output } else { v })),
        );
    }
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut cookie = None;
    let mut contexts = BTreeMap::<i32, Vec<u8>>::new();
    let mut send_contexts = HashSet::new();
    let mut total = 0usize;
    let mut redirects = 0;
    let mut stalled = 0;
    for request_number in 0..256 {
        check(&cancel, deadline)?;
        if !http::allowed(url.as_str()) {
            return Err("YouTube returned an unsupported media host".into());
        }
        let request = p::Request {
            abr: Some(p::AbrState {
                bandwidth: Some(8_000_000),
                time: Some(tracks.iter().map(|t| t.end).min().unwrap_or(0)),
                visibility: Some(1),
                rate: Some(1.),
                tracks: Some(3),
                state: Some(1),
            }),
            selected: tracks
                .iter()
                .filter(|t| t.initialized)
                .map(|t| t.id.clone())
                .collect(),
            ranges: tracks.iter().filter_map(Track::range).collect(),
            config: Some(config.clone()),
            audio: vec![tracks[1].id.clone()],
            video: vec![tracks[0].id.clone()],
            streamer: Some(p::Streamer {
                client: Some(p::ClientInfo {
                    name: Some(1),
                    version: Some(version.into()),
                }),
                cookie: cookie.clone(),
                contexts: contexts
                    .iter()
                    .filter(|(kind, _)| send_contexts.contains(*kind))
                    .map(|(kind, value)| p::ContextValue {
                        kind: Some(*kind),
                        value: Some(value.clone()),
                    })
                    .collect(),
            }),
        };
        let mut request_url = url.clone();
        request_url
            .query_pairs_mut()
            .append_pair("rn", &request_number.to_string());
        let response = client
            .post(request_url)
            .header("Content-Type", "application/x-protobuf")
            .header("Accept", "application/vnd.yt-ump")
            .body(request.encode_to_vec())
            .send()
            .map_err(|_| "YouTube streaming was interrupted")?;
        if !response.status().is_success() {
            return Err(format!(
                "YouTube refused the stream, HTTP {}",
                response.status().as_u16()
            ));
        }
        let before = total;
        let mut response = response.take(32 * 1024 * 1024);
        let mut headers = HashMap::<u32, (p::Header, Vec<u8>)>::new();
        let mut backoff = 0;
        while let Some((kind, data)) =
            read_part(&mut response).map_err(|_| "Invalid YouTube media response")?
        {
            check(&cancel, deadline)?;
            match kind {
                20 => {
                    let header = decode::<p::Header>(&data)?;
                    if header.compression.unwrap_or(0) != 0
                        || header.length.unwrap_or(0) > MAX_PART as i64
                        || headers.len() >= 32
                    {
                        return Err("Unsupported YouTube media segment".into());
                    }
                    let id = header.id.ok_or("Missing segment ID")?;
                    if headers.insert(id, (header, Vec::new())).is_some() {
                        return Err("Duplicate YouTube segment ID".into());
                    }
                }
                21 => {
                    let id = u32::from(*data.first().ok_or("Empty media segment")?);
                    let (_, bytes) = headers.get_mut(&id).ok_or("Missing media segment header")?;
                    if bytes.len() + data.len() - 1 > MAX_PART || total + data.len() - 1 > MAX_MEDIA
                    {
                        return Err("YouTube clip exceeds the 128 MB loading limit".into());
                    }
                    total += data.len() - 1;
                    bytes.extend_from_slice(&data[1..]);
                }
                22 => {
                    let id = u32::from(*data.first().ok_or("Empty media segment end")?);
                    let (header, bytes) =
                        headers.remove(&id).ok_or("Missing media segment header")?;
                    if header
                        .length
                        .is_some_and(|length| length >= 0 && length as usize != bytes.len())
                    {
                        return Err("Incomplete YouTube media segment".into());
                    }
                    let track = tracks
                        .iter_mut()
                        .find(|t| t.id.itag == header.itag)
                        .ok_or("Unexpected YouTube media format")?;
                    track.record(&header, bytes)?;
                }
                35 => {
                    let policy = decode::<p::Policy>(&data)?;
                    cookie = policy.cookie;
                    backoff = policy.backoff.unwrap_or(0).max(0);
                }
                42 => {
                    let init = decode::<p::Init>(&data)?;
                    let track = tracks
                        .iter_mut()
                        .find(|t| Some(&t.id) == init.format.as_ref())
                        .ok_or("Unexpected YouTube stream initialization")?;
                    track.initialized = true;
                    if !init
                        .end_segment
                        .is_some_and(|end| (1..=100_000).contains(&end))
                    {
                        return Err("Invalid YouTube segment count".into());
                    }
                    track.final_segment = init.end_segment;
                }
                43 => {
                    redirects += 1;
                    if redirects > 5 {
                        return Err("Too many YouTube stream redirects".into());
                    }
                    url = reqwest::Url::parse(
                        &decode::<p::Redirect>(&data)?
                            .url
                            .ok_or("Missing YouTube redirect")?,
                    )
                    .map_err(|_| "Invalid YouTube redirect")?;
                }
                44 => return Err("YouTube returned a streaming error".into()),
                57 => {
                    let update = decode::<p::ContextUpdate>(&data)?;
                    let kind = update.kind.ok_or("Missing streaming context type")?;
                    let value = update.value.unwrap_or_default();
                    if contexts.len() >= 32 || value.len() > 64 * 1024 {
                        return Err("YouTube streaming context exceeds the limit".into());
                    }
                    if update.write_policy != Some(2) || !contexts.contains_key(&kind) {
                        contexts.insert(kind, value);
                    }
                    if update.send == Some(true) {
                        send_contexts.insert(kind);
                    }
                }
                59 => {
                    let policy = decode::<p::ContextPolicy>(&data)?;
                    send_contexts.extend(policy.start);
                    for kind in policy.stop {
                        send_contexts.remove(&kind);
                    }
                    for kind in policy.discard {
                        send_contexts.remove(&kind);
                        contexts.remove(&kind);
                    }
                }
                58 => {
                    let protection = decode::<p::Protection>(&data)?;
                    if protection.status.is_some_and(|status| status >= 3) {
                        return Err("YouTube requires client verification for this video".into());
                    }
                }
                _ => {}
            }
        }
        if !headers.is_empty() {
            return Err("YouTube ended an incomplete segment".into());
        }
        if tracks.iter().all(Track::complete) {
            let [video, audio] = tracks;
            return Ok(Prepared {
                video: video.finish(&cancel, true)?,
                audio: audio.finish(&cancel, false)?,
            });
        }
        stalled = if total == before { stalled + 1 } else { 0 };
        if stalled >= 3 {
            return Err("YouTube stopped providing media data".into());
        }
        if backoff > 10_000 {
            return Err("YouTube requested a longer retry delay, try again later".into());
        }
        let wait_until = Instant::now() + Duration::from_millis(backoff as u64);
        while Instant::now() < wait_until {
            check(&cancel, deadline)?;
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    Err("YouTube clip exceeded the request limit".into())
}

fn segment_time(header: &p::Header) -> Result<(i64, i64), String> {
    let range = header.time_range.as_ref();
    let ticks = |value: Option<i64>| -> Option<i64> {
        value?
            .checked_mul(1000)?
            .checked_div(i64::from(range?.timescale?))
    };
    let start = header
        .start
        .or_else(|| ticks(range?.start_ticks))
        .ok_or("Missing media timestamp")?;
    let duration = header
        .duration
        .or_else(|| ticks(range?.duration_ticks))
        .ok_or("Missing media duration")?;
    Ok((start, duration))
}
fn check(cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Video loading cancelled".into())
    } else if Instant::now() >= deadline {
        Err("YouTube clip loading timed out".into())
    } else {
        Ok(())
    }
}
fn json_after(text: &str, marker: &str) -> Option<Value> {
    let start = text.find(marker)? + marker.len();
    serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Value>()
        .next()?
        .ok()
}
fn decode<T: Message + Default>(data: &[u8]) -> Result<T, String> {
    T::decode(data).map_err(|_| "Invalid YouTube streaming metadata".into())
}
fn read_bounded(mut input: impl Read, max: usize, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut block = [0; 8192];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Video loading cancelled".into());
        }
        let count = input
            .read(&mut block)
            .map_err(|_| "YouTube response was interrupted")?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > max {
            return Err("YouTube response exceeds the size limit".into());
        }
        output.extend_from_slice(&block[..count]);
    }
}
fn read_uint(input: &mut impl Read) -> io::Result<Option<u32>> {
    let mut first = [0u8];
    if input.read(&mut first)? == 0 {
        return Ok(None);
    }
    let first = first[0];
    let width = if first < 128 {
        1
    } else if first < 192 {
        2
    } else if first < 224 {
        3
    } else if first < 240 {
        4
    } else {
        5
    };
    if width == 1 {
        return Ok(Some(u32::from(first)));
    }
    let mut tail = [0u8; 4];
    input.read_exact(&mut tail[..width - 1])?;
    let tail = u32::from_le_bytes(tail);
    Ok(Some(if width == 5 {
        tail
    } else {
        (u32::from(first) & (127 >> (width - 1))) | tail << (8 - width)
    }))
}
fn read_part(input: &mut impl Read) -> io::Result<Option<(u32, Vec<u8>)>> {
    let Some(kind) = read_uint(input)? else {
        return Ok(None);
    };
    let size = read_uint(input)?.ok_or(io::ErrorKind::UnexpectedEof)? as usize;
    if size > MAX_PART {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut data = vec![0; size];
    input.read_exact(&mut data)?;
    Ok(Some((kind, data)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn out_of_order_segments_become_contiguous_and_gaps_fail() {
        let mut track = Track::new(&serde_json::json!({"itag":140,"lastModified":"1"})).unwrap();
        let header = |sequence, start| p::Header {
            sequence: Some(sequence),
            start: Some(start),
            duration: Some(1000),
            ..Default::default()
        };
        track.record(&header(2, 1000), vec![2]).unwrap();
        assert_eq!(track.last, 0);
        track.record(&header(1, 0), vec![1]).unwrap();
        assert_eq!(track.last, 2);
        assert_eq!(track.end, 2000);
        track.record(&header(1, 0), vec![1]).unwrap();
        assert_eq!(track.segments.len(), 2);
        assert!(track.record(&header(3, 3000), vec![3]).is_err());
    }
    #[test]
    fn ump_bounds_and_truncation_are_checked_before_allocation() {
        let too_large = [20, 240, 1, 0, 128, 0];
        assert!(read_part(&mut &too_large[..]).is_err());
        for bytes in [&[20u8][..], &[20, 3, 1, 2], &[128]] {
            assert!(read_part(&mut &bytes[..]).is_err());
        }
        assert!(read_part(&mut &[][..]).unwrap().is_none());
        assert_eq!(
            read_part(&mut &[21, 3, 0, 1, 2][..]).unwrap(),
            Some((21, vec![0, 1, 2]))
        );
    }
    #[test]
    fn ump_variable_width_integer_boundaries() {
        for (bytes, expected) in [
            (&[127][..], 127),
            (&[128, 2][..], 128),
            (&[192, 0, 2][..], 16384),
            (&[224, 0, 0, 2][..], 2097152),
            (&[240, 255, 255, 255, 255][..], u32::MAX),
        ] {
            assert_eq!(read_uint(&mut &bytes[..]).unwrap(), Some(expected));
        }
    }
    #[test]
    fn invalid_media_time_does_not_overflow_or_divide_by_zero() {
        let mut header = p::Header {
            time_range: Some(p::TimeRange {
                start_ticks: Some(0),
                duration_ticks: Some(1),
                timescale: Some(0),
            }),
            ..Default::default()
        };
        assert!(segment_time(&header).is_err());
        header.time_range.as_mut().unwrap().timescale = Some(1000);
        header.time_range.as_mut().unwrap().duration_ticks = Some(i64::MAX);
        assert!(segment_time(&header).is_err());
    }
}
