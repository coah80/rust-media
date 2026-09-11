# Rust Media

A production-ready Rust video library for the documented Windows playback contract, with an optional native Slint player. The API is pre-1.0 and may change. It resolves supported video links to media streams, reads MP4 data in byte ranges, decodes H.264 in Rust, plays AAC through Rodio/Symphonia, and gives the UI RGBA frames.

There is no WebView, browser engine, mpv, FFmpeg runtime, or Python helper. YouTube URL transformation uses Boa, a JavaScript interpreter written in Rust, with a Rust SWC preprocessor. No Node, Deno, V8 or QuickJS runtime is used. Windowing, graphics, audio output, TLS and other platform services still use their normal native dependencies. This is not a claim that every dependency or operating-system component is written in Rust.

![Native player with a generated test pattern](docs/player.png)

## What works

| Input | Current result |
| --- | --- |
| Local H.264 MP4 with optional AAC | Video, audio, pause, seek and replay verified on Windows |
| Direct MP4 from supported media hosts | Bounded HTTP range reads and native decoding |
| FixupX / FxTwitter public video posts | Public metadata adapter and native playback verified |
| YouTube | Progressive native H.264/AAC playback up to 720p, with a real buffered bar and SABR fallback |
| HTML, JavaScript widgets, arbitrary iframes | Not implemented |

The demo has volume, mute, fullscreen, keyboard controls, a seek bar, and controls that hide during playback. It is a standalone experiment, not a Discord player parity claim or a completed Fastcord integration.

## Production contract

Windows x64 playback is production-ready for supported local files, allowlisted direct MP4 URLs, public FixupX posts and public YouTube videos that resolve to H.264/AAC within the limits below. Supported inputs must fail with an error instead of substituting media or bypassing a limit. Provider availability is outside the library's control, and macOS/Linux remain preview targets until current device playback passes there.

Production-ready here covers bounded loading, deterministic decode and seek output, cancellation, replay, replacement, audio playback, the public `Player` API and both Windows Slint renderers. It does not expand the codec, container, provider or duration contract.

## Run

Requires Rust 1.92 or newer. Windows uses the MSVC toolchain. Linux additionally needs native windowing and ALSA development packages, for example `libasound2-dev`, `libfontconfig1-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`, `libx11-xcb-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libegl1-mesa-dev` and `libgl1-mesa-dev` on Ubuntu. macOS needs Xcode command-line tools. Linux execution has not been verified.

```sh
cargo run --locked --features native -- path/to/video.mp4
cargo run --locked --features native -- https://fixupx.com/user/status/POST_ID
cargo run --release --locked --features native -- https://www.youtube.com/watch?v=jNQXAC9IVRw
cargo run --locked --features native -- --probe path/to/video.mp4
cargo run --release --locked --features native -- --probe-all https://www.youtube.com/watch?v=Gf-fCJ6TkRU
```

Run without an argument to paste a file path or supported link. Space toggles playback, Left/Right seek five seconds, M mutes, F toggles fullscreen, and Escape exits fullscreen. `--probe` resolves the complete input and decodes up to 60 frames without opening a window or playing audio. `--probe-all` decodes every video frame and AAC sample. Probe error output omits source URLs and response bodies.

Set `SLINT_BACKEND` to `winit-software` or `winit-femtovg` to select a renderer. Release builds use `cargo build --release --locked --features native`.

## Library

The default library build has no Slint dependency. Enable `native` only for the demo. Git installation is supported; registry publication remains disabled while the script preprocessor uses a pinned Git dependency. Pin a reviewed commit with `rev` for reproducible integration.

```toml
[dependencies]
rust-media = { git = "https://github.com/coah80/rust-media", branch = "research/youtube-resolver" }
```

```rust
use rust_media::Player;

let player = Player::new();
player.load("clip.mp4".into());
player.pause(true);
player.seek(12.0);
player.set_volume(0.5);
let snapshot = player.snapshot();
```

Keep the player alive and poll `snapshot()` from the application's event loop. Each snapshot takes the newest available RGBA frame and includes `buffered`, the contiguous cached fraction used by the grey seek-bar layer. Older undisplayed frames are replaced. `Player` owns one worker, replacing queued loads with the newest request. Dropping or stopping it requests cancellation; dropping does not synchronously join ongoing network work. Provider metadata and SABR requests observe cancellation during both header and body waits. Live YouTube-to-local replacement took 20-40 ms. Direct-media range reads use blocking requests with finite timeouts.

`providers::resolve` returns stream addresses or prepared in-memory media. Use `MediaReader::resolved` to open either representation. `resolve_with_cancel` accepts a shared cancellation flag. `decode::MediaVideo` accepts ordinary or fragmented MP4 and emits timestamped frames. `http::RemoteFile` implements `Read + Seek` with 512 KiB blocks; progressive readers share a bounded cache and prefetch in the background. The player coordinates these parts and uses the audio playback clock when audio exists.

## Current limits

- H.264 in ordinary MP4/MOV containers, direct fragmented MP4 from YouTube and assembled SABR fragments. WebM, VP9, AV1, HEVC, HLS, DRM, subtitles and live streams are not implemented.
- Software video decoding, up to 1920 pixels on either input dimension. Output is capped at 1280×720. There is no hardware decoding or HDR/color-management pipeline; conversion currently uses limited-range BT.601.
- File limit 2 GiB, compressed-sample limit 8 MiB and a 16-frame reorder queue. A server ignoring Range is accepted only for files up to 32 MiB. Ordinary range reads retain one block per reader. MP4 metadata and codec reference frames require additional memory.
- Simple single-segment MP4 edits are supported. Multiple edit segments are rejected. There is no adaptive bitrate selection. Direct range reads retry an interrupted block twice; HTTP 4xx responses stop immediately. SABR respects bounded server-requested backoff.
- HTTPS media hosts are explicitly allowed in `http::allowed`; every redirect is checked again. Arbitrary website and local-network URLs are rejected. No account cookies or credentials are used. Anonymous provider cookies remain in memory for one resolver session. Local file paths must be supplied explicitly.
- FixupX chooses the first video in a post. Other videos in the same post are not exposed in the demo.
- YouTube opens direct H.264/AAC media through YouTube's VisionOS client profile, starts after its metadata and first blocks arrive, and fills a shared cache while playback runs. Combined direct media is limited to 128 MiB and declared duration to 20 minutes. The SABR fallback still prepares the complete clip before playback. No verification-token generator or browser fallback is included. Provider behavior can change. See [YouTube support and limits](docs/youtube.md).

## Validation

The [Windows stress and memory benchmarks](docs/benchmark-2026-09-11.md) record repeated startup, complete decodes, seek integrity and native control checks. They also document the allocator and long-playback decoder retention fixes found during testing.

```sh
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps
cargo check --locked --no-default-features --lib
```

The benchmark report covers 53,913 frames across four complete videos, a current 21,313-frame rerun of the longest video, repeated startup, deterministic stress and both Windows renderers. Listening quality, perceptual lip sync, sustained bandwidth starvation and this revision on macOS/Linux remain unverified. Hosted CI is configured for Windows, macOS and Linux builds and synthetic tests; that does not establish device playback quality.

See [contributing](CONTRIBUTING.md) for validation and release requirements. `cargo run --release --locked --example play -- clip.mp4` runs the playback worker with audio while discarding video frames. Generate API docs with `cargo doc --no-deps`.

## References

- [rusty_h264](https://github.com/remade-with-rust/rusty_h264), the Rust H.264 decoder used here with assembly disabled.
- [Rodio](https://github.com/RustAudio/rodio) and [Symphonia](https://github.com/pdeljanov/Symphonia), audio playback and Rust AAC decoding.
- [FxEmbed status API](https://github.com/FxEmbed/FxEmbed/wiki/Status-Fetch-API), the public FixupX metadata contract.

The next steps are direct-client fallback maintenance, adaptive quality selection, additional Rust codecs, and hardware decoding through native platform APIs. Rendering general web pages would be a separate project.

## License

Original project code is [MIT](LICENSE). Dependencies retain their own terms, including optional Slint licensing; see [third-party components](THIRD_PARTY.md). The native demo uses [Slint](https://slint.dev).
