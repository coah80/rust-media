# Streamable

Pass a public `https://streamable.com/<id>` link to `Player::load`. The `/e/<id>` and `/o/<id>` embed forms work too. Rust Media requests fresh metadata from `https://api.streamable.com/videos/<id>` and selects an available MP4 rendition. No page scripts or iframe run in the app.

The resolver prefers the highest-resolution rendition that fits 1280 × 720, including portrait video. If none fits, it selects a rendition within the decoder's 1920-pixel input limit. It checks dimensions and reported file size before loading. The decoder still validates the actual media, including H.264 video, AAC audio, dimensions, and resource limits.

Media uses ordinary HTTP range playback with one 512 KiB block retained per reader. It does not use YouTube's prefetch cache or SABR fallback, and does not write a video file to disk. The current buffered-progress value for this path is not a measurement of how much of the whole clip has downloaded.

Metadata requests have a 4 MiB response limit, a 20-second deadline, and cancellation. Only HTTPS media URLs on single-label `cdn-*.streamable.com` hosts are accepted. Credentials, nonstandard ports, unrelated hosts, and non-MP4 paths are rejected. Metadata redirects are not followed; media redirects must pass the existing host checks.

Processing, deleted, private, or otherwise unavailable videos may fail. HLS and unsupported codecs remain unsupported. Signed media links expire, so retain the share link and resolve it again for a later playback session.

Streamable's [API documentation](https://streamable-support.zendesk.com/hc/en-us/articles/35415672400916-API-Documentation) says native applications need pre-approval to render its signed video URLs. Integrators need to arrange that with Streamable. This implementation does not establish approval for Rust Media or apps using it.

## Windows checks, September 12, 2026

Release `--probe-all` runs decoded every video frame and AAC sample in three public clips. These are individual measurements on this machine and connection, not startup guarantees.

| Clip | Dimensions | Duration | Video frames | AAC samples | First frame | Decode wall time |
| --- | --- | --- | --- | --- | --- | --- |
| `hn8hq` | 1280 × 720 | 23.991 s | 719 | 2,117,632 | 1.216 s | 19.973 s |
| `dnd1`, using `/e/` | 1280 × 720 | 61.467 s | 1,844 | 2,709,504 | 0.834 s | 32.938 s |
| `moo` | 852 × 480 | 12.013 s | 360 | 1,155,072 | 0.340 s | 2.415 s |

Run a check with:

```sh
cargo run --release --locked --features native -- --probe-all https://streamable.com/hn8hq
```

Live provider checks stay out of CI. Permanent synthetic tests cover ID parsing, untrusted media destinations, processing status, and oversized renditions. These checks protect the network and resource boundaries when provider metadata changes.

A separate Rust consumer using the local library played `moo` to the end with 360 frames. A second run through `/o/moo` checked pause, seeking forward to 8 seconds, seeking backward to 1 second, stopping, and loading again. Audio output was muted; these runs did not assess listening quality or perceptual audio/video sync. macOS and Linux playback were not tested for this change.
