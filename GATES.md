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
  EVIDENCE: automatic-evidence=v1; definition-sha256=578d9cc43ef5ec061bbf25c43b2ef110d524950cb4772d5c5eb69f9ca1eeebd1; exit=0; EXPECT=matched; output-sha256=af9988ad10c5050e83853addae3ba066338691ea3b6cdddf696f89d8611223a7; output-bytes=2726; shell=C:\WINDOWS\system32\cmd.exe; cwd=C:\Users\cole\projects\rust-media; path=aee1c3638993/51 entries
- [ ] Y5: reviewed implementation and exact release build are published for review
  EVIDENCE: Pending final build, source publication and PR update.
- [ ] Y6: longer YouTube video plays completely through client verification
  EVIDENCE: Unmet. aqz-KE-bpKQ delivered 62.2 seconds of video and 69.9 seconds of audio, then protection status 3 stopped delivery. The implemented transport now fails immediately on that status. Inspected verification integration uses Deno/V8 and was not added under the Rust-only constraint. This gate is not covered by the successful short clip.
