# Third-party components

The MIT license covers this repository's original code. Dependencies retain their own licenses. Review their notices when redistributing a build; this overview is not a complete transitive license bundle.

| Component | Purpose | Declared license |
| --- | --- | --- |
| rusty_h264 0.16.0 | H.264 software decoding | BSD-2-Clause |
| mp4 0.14 | MP4 container parsing | MIT |
| Rodio | Audio output | MIT OR Apache-2.0 |
| Symphonia | AAC/container decoding | MPL-2.0 |
| Boa | JavaScript interpretation in Rust | Unlicense OR MIT |
| ytdlp-ejs | Script preprocessing | MIT |
| Slint, optional `native` feature | Native demo UI | GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0 |
| lucide-slint, optional `native` feature | UI icons | (MIT OR Apache-2.0) AND ISC |

Slint's licensing choices and attribution requirements are described in its [license documentation](https://github.com/slint-ui/slint/blob/master/LICENSE.md). This project does not relicense Slint under MIT. The default library build does not enable Slint.

See `Cargo.lock` for exact versions and source revisions. `cargo metadata --locked --format-version 1` exposes dependency license declarations. Synthetic MP4 fixtures were generated for this project; fetched provider media and extracted scripts are not distributed here.
