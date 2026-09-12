# Contributing

Rust Media is pre-1.0. Keep changes focused and include a reproducer for playback bugs. Report OS, Rust version, enabled features, container/codec and sanitized errors. Do not attach credentials, signed media URLs or private recordings. Use generated fixtures for regression tests.

## Checks

```sh
cargo fmt --all -- --check
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps
cargo run --release --locked --features native -- --probe-all tests/fixtures/pattern.mp4
```

Ubuntu needs `libasound2-dev` and `pkg-config` for the library. The demo additionally needs `libfontconfig1-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`, `libx11-xcb-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libegl1-mesa-dev` and `libgl1-mesa-dev`. Windows needs MSVC; macOS needs Xcode command-line tools.

Validate UI changes on both `winit-software` and `winit-femtovg`. Offline CI covers synthetic media, decoding integrity, resource limits, cancellation and destination restrictions. Provider availability is checked manually so external rate limits do not make routine CI nondeterministic.

## Release scope

Git consumption is supported. `publish = false` prevents accidental registry publication while the pinned Git-only script dependency remains. The registry has ytdlp-ejs 0.1.1, but its source differs from the tested Git revision; it is not a verified drop-in replacement. A crates.io release requires resolving that dependency, verifying the packaged archive and reviewing the public API. Do not remove the guard just to make publishing succeed.

Before expanding production support beyond the documented Windows contract, validate long real-time sessions, slow/disconnected networks, seek during buffering, audio device changes, audio/video sync and playback on the added platform. Record measurements and failures. No benchmark establishes flawless behavior for every input.
