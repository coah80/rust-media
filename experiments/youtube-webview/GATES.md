# Gates: YouTube webview experiment

OWNS: experiments/youtube-webview/**

Scope: play YouTube using a Rust browser engine with measured low memory usage. A Rust host around a non-Rust engine is explicitly rejected. A working DOM demonstration does not complete the playback goal.

- [x] W1: the selected browser and JavaScript engines are Rust
  EVIDENCE: Locked Blitz, Boa and Vello CPU dependencies. No WebView2, Chromium, SpiderMonkey or V8. Native OS APIs remain in use; this is not a whole-system language claim.
- [ ] W2: the longer YouTube clip plays past the previous cutoff and reaches its end
  EVIDENCE: UNMET. This prototype has no HTML video, iframe, networking or MediaSource implementation. No YouTube playback claim.
- [ ] W3: memory measurement includes idle, playing and unloaded states
  EVIDENCE: PARTIAL. Full process tree measured for the tiny DOM page: 8.21 MiB private resident idle, 8.24 MiB after 11 clicks. Playback memory remains unmeasured. See README for commit versus working-set distinctions.
- [x] W4: the experiment has a reproducible build and launch command
  EVIDENCE: Windows optimized development build, format check and strict all-target Clippy passed. Locked dependencies and commands in README. Other operating systems remain unverified.
- [x] W5: native input executes JavaScript and repaints a DOM mutation
  EVIDENCE: Native window messages through Winit/Blitz, captured counter values 0, 1 and 11. This is not physical mouse or YouTube verification.
