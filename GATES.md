# Gates: native video experiment

OWNS: src/**, ui/**, tests/**, docs/**, Cargo.toml, Cargo.lock, build.rs, README.md, GATES.md, .gitignore

Scope: an independent Rust media library and native player, with tested direct MP4 playback and measured provider limitations.

- [x] G1: parser, request and playback-state regression tests pass
  CHECK: cargo test --locked
  EXPECT: test result: ok
  EVIDENCE: automatic-evidence=v1; definition-sha256=903084afb67f2771e68383a3d2e415500fbb4dba1c69b3470044e5cd7d5c3ac5; exit=0; EXPECT=matched; output-sha256=27ed1162e668c3b0ab3832ec12b797ab5625a0cef6f1598886f7d1f6644dc746; output-bytes=1498; shell=C:\WINDOWS\system32\cmd.exe; cwd=C:\Users\cole\projects\rust-media; path=aee1c3638993/51 entries

- [x] G2: all targets pass strict lint
  CHECK: cargo clippy --locked --all-targets -- -D warnings && echo CLIPPY_OK
  EXPECT: CLIPPY_OK
  EVIDENCE: automatic-evidence=v1; definition-sha256=d24b69f7a74027f6a4c44c7722f929a514b7c2bffbf54ead597c5dce8d226942; exit=0; EXPECT=matched; output-sha256=60a6548e79d7d723f31ea7c2179bd8ed5b32bf91b39d80507b5c92f9f36699a6; output-bytes=151; shell=C:\WINDOWS\system32\cmd.exe; cwd=C:\Users\cole\projects\rust-media; path=aee1c3638993/51 entries

- [x] G3: native playback and controls verified with both renderers
  EVIDENCE: Windows native event-dispatch checks passed with winit-software and winit-femtovg. Generated H.264/AAC fixture exercised pause, paused seek, resume, mute, fullscreen, Escape, resize, EOF and replay. Final visual correction confirmed with software; same controls passed on live FixupX with femtovg. Screenshots inspected. Temporary driver removed before final source checks.

- [x] G4: live provider probes recorded without claiming unsupported playback
  EVIDENCE: FixupX resolved and decoded 60 frames at 482x360, 19.034-second duration, then passed native playback controls with audio. Follow-up public YouTube probes showed SABR-only metadata; the prior JavaScript diagnosis was incorrect. The corrected adapter reports unsupported SABR streaming, and docs/youtube.md records both failed public-video probes and rejected native-client requests. Local 720p probe decoded 60 frames in 0.786 seconds. No private responses or stream URLs committed.

- [x] G5: independent GitHub repository contains the reviewed source and reproducible instructions
  EVIDENCE: Private repository https://github.com/coah80/rust-media created and source pushed. GitHub main SHA matched local initial source commit e39fccd0dd91e678d8797a554e3d8d5b666711e2. README includes run commands, library usage, a generated-fixture screenshot, measured results and explicit YouTube/platform limitations. Final build and UI-free library check passed.
