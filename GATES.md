# Gates: native YouTube playback

OWNS: src/**, ui/**, tests/**, docs/**, Cargo.toml, Cargo.lock, README.md, GATES.md

Scope: native YouTube video/audio playback and controls, with broader provider verification tracked separately.

- [x] Y1: complete public YouTube media decodes through Rust
  EVIDENCE: Live full-clip check decoded all 284 ordered frames of jNQXAC9IVRw, 18.933 seconds of video and 19.064 seconds of AAC. No browser or external downloader.
- [x] Y2: native audio/video playback, bounded loading and cancellation are verified
  EVIDENCE: Active audio output drives the video clock; complete AAC decoding verified. Media loading capped at 128 MiB. Cancelling during URL resolution and replacing with a local file completed in 0.020 seconds after the resolver wait fix. One script worker at a time. Listening quality and perceptual lip sync unverified.
- [x] Y3: native controls work on both renderers
  EVIDENCE: Live YouTube passed pause across frames, paused seek, resume, mute, fullscreen, Escape, resize, EOF, replay and replacement on winit-software and winit-femtovg. Live FixupX passed the same controls. Temporary native event driver removed.
- [x] Y4: regression tests and strict lint pass on the final source
  CHECK: cargo test --locked && cargo clippy --locked --all-targets -- -D warnings && echo YOUTUBE_CHECKS_OK
  EXPECT: YOUTUBE_CHECKS_OK
  EVIDENCE: automatic-evidence=v1; definition-sha256=578d9cc43ef5ec061bbf25c43b2ef110d524950cb4772d5c5eb69f9ca1eeebd1; exit=0; EXPECT=matched; output-sha256=bc93430f00c43f500238ba86cec9b36e568244632e9020e21c430475d53120a0; output-bytes=3323; shell=C:\WINDOWS\system32\cmd.exe; cwd=C:\Users\cole\projects\rust-media; path=33d0d78d2ad2/51 entries
- [x] Y5: reviewed source is published and the exact Windows release build is verified
  EVIDENCE: Draft PR https://github.com/coah80/rust-media/pull/1 contains the transport and direct-client fixes with measured limits. Local target/release/rust-media.exe SHA256 is 992c40ca01c73c54ad8725a8b217f92898b811ddff15f4229a3009a4648656f1. The final release rebuild and complete long-video probe passed; FixupX and the short YouTube control passed earlier. The binary is local, not a GitHub release asset.
- [x] Y6: longer YouTube video plays completely through client verification
  CHECK: target\release\rust-media.exe --probe-all https://www.youtube.com/watch?v=Gf-fCJ6TkRU && echo LONG_YOUTUBE_OK
  EXPECT: LONG_YOUTUBE_OK
  EVIDENCE: automatic-evidence=v1; definition-sha256=e027e52f3c9394041356d0829c97429e4c69a8e960722639700e1969d4ea8553; exit=0; EXPECT=matched; output-sha256=e455c8f87afe2fbe54e6f05bccd4bab94ba89b6665e8ea1bb06edab1d7d86328; output-bytes=174; shell=C:\WINDOWS\system32\cmd.exe; cwd=C:\Users\cole\projects\rust-media; path=33d0d78d2ad2/51 entries
- [x] Y7: cancelling stalled provider, script or media requests releases playback promptly
  EVIDENCE: Production provider fetch tests failed before the fix and passed afterward against stalled-header and stalled-body local servers, each with a 500 ms cancellation bound. Script and SABR requests now use the same request path. Live YouTube-to-local replacement reached Playing with a frame in 0.040 seconds after early cancellation and 0.020 seconds after later cancellation. Direct-file range reads remain blocking and are outside this gate.
- [x] Y8: verified transport defects preserve context and reject inconsistent media
  EVIDENCE: Full-table context refresh and beyond-end media tests failed before the fixes and passed afterward. Full live short-clip regression decoded 284 frames and 19.064 seconds of audio. Those protocol fixes remain covered independently of the direct-media client path added for Y6.
