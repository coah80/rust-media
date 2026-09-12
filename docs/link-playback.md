# Video links and playback timing

YouTube watch, short, and embed links accept `t=1479`, `t=24m39s`, `start=1479`, or `#t=24m39s`. Tracking parameters do not affect the start time. An invalid timestamp falls back to zero; a valid timestamp beyond the media duration returns an error. Parsed times are bounded to seven days.

A `/clip/<id>` link resolves the source video and the clip's millisecond boundaries from the public clip page. The response must match the requested clip ID. The player starts at the clip's first timestamp, stops at its end, and displays a timeline relative to that segment. Seeking stays inside the clip. Playback stops at the end rather than looping automatically.

`providers::Resolved` now includes `start_time` and `end_time`. Apps using `Player` get timing automatically. Apps constructing `Resolved` directly must supply these fields; use `0.0` and `None` for ordinary playback. Apps decoding frames directly must apply the timing themselves. The bundled probe now honors both fields and reports its first presentation timestamp.

Long YouTube videos use range reads instead of prefetching the whole video into RAM. The direct path allows recorded videos up to seven days, subject to the existing 2 GiB per-file, codec, dimension, and container limits. This does not increase the SABR fallback's 20-minute limit. There is no long-video SABR fallback or automatic selection of a smaller rendition when the selected stream exceeds the file-size limit.

## Other link formats

Individual Imgur video links and `i.imgur.com/<id>.gifv` links resolve to `i.imgur.com/<id>.mp4`. Image-only posts, albums, and galleries are not supported.

GIPHY `/gifs/<slug>-<id>` and `/embed/<id>` links use the MP4 rendition at `media.giphy.com/media/<id>/giphy.mp4`. Direct MP4 links on its numbered media CDN hosts also work. These are ordinary video playback, with no sticker transparency or automatic GIF looping. GIPHY documents its MP4 renditions in the [API schema](https://developers.giphy.com/docs/api/schema/).

These adapters construct URLs on fixed hosts from validated IDs. They do not fetch arbitrary HTML or execute page scripts. The decoder still checks actual media format and size. Provider changes, removed media, unsupported codecs, and access restrictions can cause playback failures.

## Windows checks, September 12, 2026

| Input | Result |
| --- | --- |
| YouTube clip `Ugkx-nkAqE5n3oaFP2PuvOzXfjoV8IxB21fB` | Source `4YJKc8PVGio`, first PTS 1449.933 s, end 1462.563 s. Full segment decode produced 379 frames and 1,114,051 AAC samples. |
| `youtu.be/4YJKc8PVGio?t=1479` | First PTS 1479.000 s; the first 60 frames decoded. Source duration 10412.233 s. |
| Imgur `owLfF25.gifv` | All 450 frames decoded at 720 × 404, duration 15.015 s, no audio track. |
| GIPHY `cZ7rmKfFYOvYI` | All 17 frames decoded at 480 × 300, duration 1.531 s, no audio track. |

A separate app using `Player` played the entire YouTube clip and reported a 12.630-second timeline, then stopped. The timestamp link began at position 1479.000 and delivered 60 frames. Audio output was active at zero volume. These checks do not establish listening quality or perceptual lip sync.

The same consumer played Imgur and GIPHY to the end with 450 and 17 frames respectively. The local suite passed 78 checks, including the doctest. Clippy across all targets and features and formatting checks passed.

Decoder-only first-frame times were 5.045 seconds for the clip and 3.023 seconds for the timestamp link. The real player took 16.207 and 15.265 seconds respectively. Symphonia 0.5.5's fragmented MP4 audio seek walks earlier segments, so deep audio seeks remain a startup bottleneck. These are individual runs on this machine and connection, not startup guarantees.

Permanent synthetic checks cover clip ID and time validation, bounded timestamps, fixed media destinations, and publishing frame pixels together with the corresponding timeline state. Live network checks stay out of CI. macOS and Linux playback were not exercised for this change.

Further provider adapters, HLS, additional codecs, and faster indexed AAC seeking remain separate work. Supporting these URL forms does not mean every video on those sites can play.
