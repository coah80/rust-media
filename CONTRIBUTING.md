# Contributing

Bug fixes, playback improvements, and clearer docs are welcome. Keep each pull request focused on one change and explain how you tested it. The API is still pre-1.0, so call out any changes that would affect apps using the library.

## Report a bug

Include the steps to reproduce it, what you expected, and what happened. Tell us your OS, Rust version, enabled Cargo features, and the video's container and codecs if you know them.

A small generated video that shows the problem is useful. Public links are fine for provider issues, but remove credentials and signed media URLs from logs. Do not upload private recordings.

## Work on a fix

Reproduce the problem before changing the code. For decoding or seeking bugs, compare timestamps and frames with the original behavior. Add a regression test when it protects playback correctness, resource limits, cancellation, or data integrity.

Use generated fixtures for automated tests. Keep live YouTube and FixupX requests out of CI; availability and rate limits can change without a code change. Check those providers separately when your patch touches them.

## Checks

Requires Rust 1.92 or newer. Windows needs MSVC; macOS needs Xcode command-line tools.

Ubuntu needs `libasound2-dev` and `pkg-config` for the library. The demo also needs `libfontconfig1-dev`, `libxkbcommon-dev`, `libwayland-dev`, `libx11-dev`, `libx11-xcb-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libegl1-mesa-dev`, and `libgl1-mesa-dev`.

Run these from the repository root:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo doc --locked --no-deps
cargo run --release --locked --features native -- --probe-all tests/fixtures/pattern.mp4
```

For UI changes, try the player with `SLINT_BACKEND` set to `winit-software`, then `winit-femtovg`. Test the controls you changed while video is playing. The probe command above checks decoding without opening a window or playing sound.

Include the checks you ran and any failures in your PR. Documentation-only changes need a review of examples and links, not a full playback test run.

## Adding support

If you add a format, provider, or platform, document what works and test it through the player. Check long playback, seeking while buffering, slow or disconnected networks, audio device changes, and audio/video sync where relevant. Record the device, build, and results so someone else can repeat the test.

## Publishing

Apps currently install Rust Media from Git. `publish = false` stays in place because the script preprocessor depends on a pinned Git revision of `ytdlp-ejs`. The crates.io version has different source and needs testing before it can replace that dependency.

Before a crates.io release, resolve the Git dependency, check the packaged archive, and review the public API. Removing the publishing guard alone does not complete those steps.
