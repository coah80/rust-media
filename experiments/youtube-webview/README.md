# Rust webview lab

An independent Rust HTML/CSS/JavaScript experiment. **It does not play YouTube.**

The browser engine must be Rust. This uses Blitz 0.3.0-beta.2 for HTML, DOM and CSS, Boa 0.22.0 for JavaScript, and Vello CPU 0.17.0 through Softbuffer for rendering. It does not use WebView2, Chromium, SpiderMonkey, V8 or a GPU renderer. Window creation, font discovery and presentation call operating-system APIs. This is not a claim that the OS or every platform library is Rust.

## Run

From this directory:

```powershell
cargo build --locked
cargo run --locked
cargo run --locked -- path/to/local.html
```

The default page is a counter. Clicking its button runs a Boa callback, changes the Blitz DOM and repaints the native window. The page is retained as the runnable demonstration, not as a regression test fixture.

Only trusted local pages are suitable. The bridge implements `document.querySelector`, `textContent` and target-only click listeners. It has no event bubbling, navigation, external script loading, timers, fetch, iframe contexts, HTML media playback or MediaSource. External scripts, iframe, video and audio elements produce an explicit unsupported error. There is no browser security sandbox. HTML, text, listener and JavaScript loop/recursion limits are prototype bounds, not a hard memory or execution-time guarantee.

## Windows verification

Verified with Rust 1.98.1, locked dependencies and the optimized development build, at an 800 by 600 client size. Other platforms and release builds remain unverified.

- `cargo build --locked`, `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings` passed.
- Native window mouse messages traversed Winit and Blitz to the Boa callback. Window captures visibly showed 0, 1 and 11 on successive checks. Physical mouse input was not tested.
- No JavaScript errors were logged during those interactions.
- Unsupported media, an invalid selector and an infinite JavaScript loop each exited with code 1 and the expected error. Temporary inputs were kept outside the committed experiment.
- Executable SHA256: `686435322f45b731b3d11ed162879800bfdd2ca2b6575824d734ed77a75d7018`.

The process tree contained the lab executable and `conhost.exe`. Measurements include both:

| State | Private resident MiB | Private commit MiB | Summed working set MiB |
| --- | ---: | ---: | ---: |
| Idle after first click, 7 samples | 8.21 | 9.57 | 40.41 |
| After 11 clicks, 3 samples | 8.24 | 9.54 | 40.48 |
| After closing, 6 samples, zero processes | 0 | 0 | 0 |

These are tiny-page measurements, not YouTube estimates. Private commit is committed address space, not resident RAM. Summed working set may count shared pages more than once. GPU memory is not measured. Samples do not capture startup peaks.

```powershell
./Measure-Memory.ps1 -RootProcessId <pid> -Seconds 30 -OutputPath memory.csv
```

The sampler follows descendants and already observed orphaned descendants, checks process creation times against PID reuse, and reports how many processes had counters. Processes that start and exit between samples are not observed.

## Remaining work

The next useful slice is a local HTML video element backed by the existing Rust decoder, with play, pause and current-time events. External script loading and the DOM/event APIs required by a real player also need implementation. YouTube additionally requires compatible networking, iframe behavior, media delivery and substantial browser APIs. This counter proves none of that, and the previous longer-video cutoff remains unresolved.

The underlying projects are [Blitz](https://github.com/DioxusLabs/blitz) and [Boa](https://github.com/boa-dev/boa). [Servo uses SpiderMonkey](https://servo.org/blog/2024/04/15/spidermonkey/), so it does not meet the JavaScript-engine constraint here.
