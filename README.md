# Rust Media

An experimental Rust video library and native Slint player. It resolves supported video links to media streams, reads MP4 data in byte ranges, decodes H.264 in Rust, plays AAC through Rodio/Symphonia, and gives the UI RGBA frames.

There is no WebView, browser engine, mpv, FFmpeg runtime, or Python helper. YouTube URL transformation uses Boa, a JavaScript interpreter written in Rust, with a Rust SWC preprocessor. No Node, Deno, V8 or QuickJS runtime is used. Windowing, graphics, audio output, TLS and other platform services still use their normal native dependencies. This is not a claim that every dependency or operating-system component is written in Rust.

![Native player with a generated test pattern](docs/player.png)

## What works

| Input | Current result |
| --- | --- |
| Local H.264 MP4 with optional AAC | Video, audio, pause, seek and replay verified on Windows |
| Direct MP4 from supported media hosts | Bounded HTTP range reads and native decoding |
| FixupX / FxTwitter public video posts | Public metadata adapter and native playback verified |
| YouTube | Native H.264/AAC playback verified for short and long public clips through a direct-media client path, with SABR as a fallback |
| HTML, JavaScript widgets, arbitrary iframes | Not implemented |

The demo has volume, mute, fullscreen, keyboard controls, a seek bar, and controls that hide during playback. It is a standalone experiment, not a Discord player parity claim or a completed Fastcord integration.

## Run

Requires Rust 1.92 or newer. Windows uses the MSVC toolchain. Linux additionally needs native windowing and ALSA development packages, for example `libasound2-dev`, `libfontconfig1-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`, `libx11-xcb-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libegl1-mesa-dev` and `libgl1-mesa-dev` on Ubuntu. macOS needs Xcode command-line tools. Linux and macOS execution have not been verified.

```sh
cargo run --locked -- path/to/video.mp4
cargo run --locked -- https://fixupx.com/user/status/POST_ID
cargo run --release --locked -- https://www.youtube.com/watch?v=jNQXAC9IVRw
cargo run --locked -- --probe path/to/video.mp4
cargo run --release --locked -- --probe-all https://www.youtube.com/watch?v=Gf-fCJ6TkRU
```

Run without an argument to paste a file path or supported link. Space toggles playback, Left/Right seek five seconds, M mutes, F toggles fullscreen, and Escape exits fullscreen. `--probe` resolves the complete input and decodes up to 60 frames without opening a window or playing audio. `--probe-all` decodes every video frame and AAC sample. Probe error output omits source URLs and response bodies.

Set `SLINT_BACKEND` to `winit-software` or `winit-femtovg` to select a renderer. Release builds use `cargo build --release --locked`.

## Library

Disable default features when consuming the library without Slint. The `native` feature belongs to the demo; the playback library does not depend on its UI state.

```toml
[dependencies]
rust-media = { git = "https://github.com/coah80/rust-media", default-features = false }
```

```rust
use rust_media::player::Player;

let player = Player::new();
player.load("clip.mp4".into());
player.pause(true);
player.seek(12.0);
player.set_volume(0.5);
let snapshot = player.snapshot();
```

Keep the player alive and poll `snapshot()` from the application's event loop. Each snapshot takes the newest available RGBA frame; older undisplayed frames are replaced. `Player` owns one worker, replacing queued loads with the newest request. Dropping or stopping it cancels playback. Provider metadata, YouTube script and SABR requests observe cancellation during both header and body waits. Live YouTube-to-local replacement took 20-40 ms. Ordinary direct-file range reads still use blocking requests with finite timeouts.

`providers::resolve` returns stream addresses or prepared in-memory media. Use `MediaReader::resolved` to open either representation. `resolve_with_cancel` accepts a shared cancellation flag. `decode::Video` accepts a `Read + Seek` source and emits timestamped frames. `http::RemoteFile` implements that interface with 512 KiB reads. The player coordinates these parts and uses the audio playback clock when audio exists.

## Current limits

- H.264 in ordinary MP4/MOV containers, plus assembled fragmented MP4 from YouTube direct-media and SABR responses. WebM, VP9, AV1, HEVC, HLS, DRM, subtitles and live streams are not implemented.
- Software video decoding, up to 1920 pixels on either input dimension. Output is capped at 1280×720. There is no hardware decoding or HDR/color-management pipeline; conversion currently uses limited-range BT.601.
- File limit 2 GiB, compressed-sample limit 8 MiB and a 16-frame reorder queue. A server ignoring Range is accepted only for files up to 32 MiB. Ordinary range reads retain one block per reader. MP4 metadata and codec reference frames require additional memory.
- Simple single-segment MP4 edits are supported. Multiple edit segments are rejected. There is no adaptive bitrate selection or automatic network retry. SABR respects bounded server-requested backoff.
- HTTPS media hosts are explicitly allowed in `http::allowed`; every redirect is checked again. Arbitrary website and local-network URLs are rejected. No account cookies or credentials are used. Anonymous provider cookies remain in memory for one resolver session. Local file paths must be supplied explicitly.
- FixupX chooses the first video in a post. Other videos in the same post are not exposed in the demo.
- YouTube loads the complete clip before playback, with limits of 128 MiB received media and 20 minutes of declared duration. It first requests direct H.264/AAC media through YouTube's VisionOS client profile and falls back to the existing SABR path. No verification-token generator or browser fallback is included. Provider behavior can change. See [YouTube support and limits](docs/youtube.md).

## Validation

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo check --locked --no-default-features --lib
```

Windows checks exercised native video and AAC playback, pause across frames, seeking while paused, resume, mute, fullscreen, Escape, resizing, end-of-file and replay with both Slint renderers. A live FixupX clip passed the same interaction sequence with femtovg. The temporary UI driver was removed after verification. Audio progress was checked through the active output pipeline; listening quality was not assessed.

On the development machine, the decoder probe processed 60 generated 1280×720 H.264 frames in 0.786 seconds, including opening the file and lookahead. This is a single local probe, not a general performance benchmark. FixupX returned 60 decodable 482×360 frames with a 19.034-second duration. The supplied `Gf-fCJ6TkRU` test decoded all 4,159 H.264 frames and 15,290,368 AAC samples through 173.292 seconds in 43.094 seconds of wall time. The earlier `aqz-KE-bpKQ` verification failure now resolves and decodes through the direct client path. The short control clip still passes. A macOS arm64 release build loaded the supplied clip in the native player and reached its media title. Windows native controls previously passed with both renderers; the updated macOS controls were not independently exercised.

The small committed synthetic fixtures check frame ordering, MP4 time offsets, pixel stability after seek, fragment offsets, complete audio across interleaved video packets and malformed input handling. Protocol tests cover length bounds, timestamp overflow and script cancellation/deadlines. Provider and player tests cover destination restrictions, stale playback cancellation and invalid control values. The fixtures were generated locally, contain no account data and need no FFmpeg installation to run tests.

## References

- [rusty_h264](https://github.com/remade-with-rust/rusty_h264), the Rust H.264 decoder used here with assembly disabled.
- [Rodio](https://github.com/RustAudio/rodio) and [Symphonia](https://github.com/pdeljanov/Symphonia), audio playback and Rust AAC decoding.
- [FxEmbed status API](https://github.com/FxEmbed/FxEmbed/wiki/Status-Fetch-API), the public FixupX metadata contract.

The next steps are progressive segment playback, direct-client fallback maintenance, additional Rust codecs, and hardware decoding through native platform APIs. Rendering general web pages would be a separate project.
