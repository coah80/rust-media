# Third-party components

This project's code uses the MIT license. Its dependencies have their own licenses, listed below for the main components. If you redistribute a build, check the notices for its full dependency tree too.

| Component | Purpose | Declared license |
| --- | --- | --- |
| rusty_h264 0.16.0 | H.264 software decoding | BSD-2-Clause |
| mp4 0.14 | MP4 container parsing | MIT |
| rff-core, rff-codec, rff-format, rff-format-mp4 | Codec interfaces and MP4 packet extraction | Apache-2.0 |
| rusty_vp9 / rff-codec-vp9 | VP9 decoding | Apache-2.0 |
| rusty_av1d, assembly feature disabled | AV1 decoding | BSD-2-Clause |
| rusty_h265 / rusty_h265-accel, vendored 0.6.0 | HEVC decoding and Rust SIMD kernels | Apache-2.0 |
| matroska-demuxer | WebM and Matroska parsing | Zlib OR MIT OR Apache-2.0 |
| rusty-opus | Opus decoding | BSD-3-Clause |
| rff-codec-vorbis / lewton | Vorbis decoding | Apache-2.0 / MIT OR Apache-2.0 |
| Rodio | Audio output | MIT OR Apache-2.0 |
| Symphonia | AAC/container decoding | MPL-2.0 |
| Boa | JavaScript interpretation in Rust | Unlicense OR MIT |
| ytdlp-ejs | Script preprocessing | MIT |
| Slint, optional `native` feature | Native demo UI | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| lucide-slint, optional `native` feature | UI icons | (MIT OR Apache-2.0) AND ISC |

Slint's licensing choices and attribution requirements are described in its [license documentation](https://github.com/slint-ui/slint/blob/master/LICENSE.md). This project does not relicense Slint under MIT. The default library build does not enable Slint.

Check `Cargo.lock` for exact versions and source revisions. Run `cargo metadata --locked --format-version 1` to read dependency license declarations. The MP4 test fixtures were generated for this project. Downloaded provider media and extracted scripts are not included in the repository.

The HEVC dependency includes a local bounds-check fix. Its source, Apache license, upstream revision, and modifications are recorded in [vendor/README.md](vendor/README.md).
