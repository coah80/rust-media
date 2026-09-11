# Windows playback benchmarks, 2026-09-11

Release builds on Windows x64, AMD Ryzen 7 9800X3D. Measurements use Windows process memory counters sampled every 100 ms. Working set is resident process memory; private bytes are committed private memory. Neither includes all GPU memory. Live network timings include resolution and initial media reads. Concurrent builds and probes affected some full-decode timings, so they are correctness and memory checks, not controlled throughput comparisons.

## Production-readiness rerun

The final review run added 28 focused regressions covering all 26 Macroscope review threads and filtered correctness notes. They cover parser and cache limits, aggregate fragment limits, extended box sizes, track defaults, multiple-run rejection, fragment timelines, track-specific indexes, SAP-aware seeking, fragmented audio detection, edit lists, sparse-frame probing, split UMP parts, bounded context policies, reserved framing bytes, cancellation during stalled response headers, bodies, remuxing and retries, duration enforcement, provider error selection and Tokio runtime reentry.

The full suite passed 61 checks with native features. A separate stress test passed 100 fresh decoder open/seek cycles, 20 repeated complete ordinary decodes and 25 repeated complete fragmented decodes with exact presentation timestamps and RGBA checksums. This run found and fixed a B-frame seek boundary that the smaller seek test missed.

Twelve fresh-process YouTube startup probes covered the same four videos three times each. Every video retained its checksum. First-frame time ranged from 1.000 to 1.780 seconds. A current complete Sintel run decoded 21,313 frames and 78,329,856 AAC samples through 888.000 seconds in 192.293 seconds. Its checksum remained `4f336c142cea91b5`, resident memory stayed roughly 140-153 MiB during the run and peaked at 163.4 MiB. A current complete `Gf-fCJ6TkRU` run decoded 4,159 frames and 15,290,368 AAC samples through 173.250 seconds in 46.298 seconds with checksum `bec25ce2bd6e2572`; peak resident/private memory was 110.5/99.6 MiB.

Two public FixupX videos completed through video and audio with 464 and 908 frames. Their repeated 60-frame probes kept the same checksums. A clean external consumer built the default library without Slint and ran the real zero-volume `Player` pipeline to completion for a 192-frame local file and the 464-frame FixupX video.

A current forced SABR fallback completed `jNQXAC9IVRw` through 284 frames and 1,681,408 AAC samples at 18.933 seconds with checksum `da6ae811065c373b`. The test used the production continuation parser and a temporary fallback selector that was removed afterward. The normal progressive path remained the release default.

The reviewed Windows release executable SHA-256 is `eacf34b63b79a0de053414e5996957d0b8990964481f8787865f5c1075e7d489`.

## Problems found and fixes

The decoder's optional `global-alloc` feature installed its allocator across the entire application. On the same local 90-frame 720p clip, removing that feature reduced peak working set from 129.5 to 84.8 MiB and private bytes from 1029.8 to 74.4 MiB. Decode times were 0.959 and 0.981 seconds, with identical checksum `aa7c17764dfb791d`. Rust's normal platform allocator is now used.

That change alone did not fix sustained decoder retention. A full Sintel decode still climbed from 154.5 MiB resident at five seconds to 1224.8 MiB at 205 seconds, peaking at 1227.3 MiB resident and 1264.6 MiB private. A temporary allocation counter confirmed these were live decoder-owned allocations that were released when the decoder was reset.

Both MP4 paths now recreate the decoder at actual H.264 IDR pictures and reload the AVC configuration. IDR detection reads the length-delimited NAL headers; it does not treat every container sync flag as an IDR. The presentation queue remains intact. This releases retained decoder storage at independent picture boundaries. Streams with extremely sparse IDR pictures can still retain more memory between resets; this is not a hard process-memory cap.

Three complete passes through the cached 4,159-frame supplied video peaked at 82.8 MiB resident and 78.7 MiB private after the change, versus 225.7 and 221.4 MiB before it with the platform allocator. Live allocations at frames 1000/2000/3000/4000 were 59.4/60.9/62.4/56.1 MiB on every pass, compared with 93.9/127.4/165.3/191.3 before. Dropping the decoder released its allocations in both cases. The counter and benchmark executable were temporary diagnostics, not shipped dependencies.

## Interaction and integrity checks

The complete 14:48 Sintel probe after IDR recycling peaked at 162.2 MiB resident and 151.5 MiB private, down from 1227.3 / 1264.6 MiB with the allocator-only fix. Resident samples remained approximately 137-156 MiB through the run rather than increasing with elapsed playback. Every decoded frame matched the earlier aggregate checksum.

- 240 deterministic forward/backward indexed seeks matched linear presentation timestamps and every RGBA byte.
- Delayed reads limited to 97 bytes and an injected timeout recovered to all 48 expected fixture frames with matching pixels and timestamps. This exercises the reader boundary, not a complete simulated network outage.
- A temporary player driver completed 20 load/pause/paused-seek/resume/end/replay/replace/stop cycles through the real muted audio output pipeline, then passed another 20-cycle run. The repeated run peaked at 16.2 MiB resident and 4.3 MiB private.
- A temporary native UI driver exercised the supplied YouTube clip with both Slint renderers: frame snapshot, Space pause, position stability, paused seek to 120 seconds, resume, mute, fullscreen, Escape, resize, seek to end, replay and replacement with a local clip. Software passed in 8.937 seconds at 140.7 MiB resident / 115.0 MiB private peak. Femtovg passed in 9.173 seconds at 200.5 / 231.4 MiB. These are automated interaction checks, not listening or perceptual lip-sync assessments.
- Temporary UI and lifecycle drivers were removed. The seek and interrupted-reader tests remain because they protect decoded-data integrity.
- `cargo test --locked`: 33 tests passed. Strict all-target Clippy and the library-only no-default-features check passed.

## Startup repetition

After the allocator change, before IDR recycling, three fresh-process probes per video all passed:

| Video | First-frame range | Peak working-set range |
| --- | ---: | ---: |
| `Gf-fCJ6TkRU` | 1.431-1.501 s | 96.7-98.1 MiB |
| `aqz-KE-bpKQ` | 1.162-1.429 s | 57.7-61.1 MiB |
| `eRsGyueVLvQ` | 1.148-1.244 s | 89.5-90.5 MiB |
| `LXb3EKWsInQ` | 1.222-1.610 s | 59.8-61.6 MiB |

All twelve were progressive, decoded 60 frames, and retained each video's checksum across runs. These numbers are not guarantees for other networks or provider responses.

## Complete decode after both fixes

All four probes decoded the entire video and AAC stream, reached the final timestamp, filled the progressive cache, and matched the pre-change aggregate RGBA checksum. In total, 53,913 video frames were checked. These full probes ran with other work on the machine; use their wall times as observations, not isolated speed comparisons.

| Video | Frames | AAC samples | Last PTS | First frame | Decode wall | Peak resident / private | RGBA checksum |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `Gf-fCJ6TkRU` | 4,159 | 15,290,368 | 173.292 s | 1.470 s | 57.331 s | 109.9 / 99.5 MiB | `bec25ce2bd6e2572` |
| `aqz-KE-bpKQ` | 19,037 | 55,973,888 | 634.567 s | 1.083 s | 113.533 s | 98.1 / 87.0 MiB | `51d0ef36a46e0ca9` |
| `eRsGyueVLvQ` | 21,313 | 78,329,856 | 888.042 s | 1.268 s | 240.537 s | 162.2 / 151.5 MiB | `4f336c142cea91b5` |
| `LXb3EKWsInQ` | 9,404 | 27,680,768 | 313.780 s | 1.279 s | 82.825 s | 86.8 / 75.4 MiB | `032462b4ba44a69e` |

Run a complete probe with `cargo run --release --locked --features native -- --probe-all https://www.youtube.com/watch?v=VIDEO_ID`. Use `--probe` for startup plus up to 60 frames. Probes do not present frames in real time or play sound.

After removing the temporary UI driver, the release executable was rebuilt and all four startup probes passed again in sequence. First-frame times were 1.594 / 1.396 / 1.305 / 1.346 seconds in the table's video order; their 60-frame checksums matched the earlier runs. The local 90-frame 720p probe also retained checksum `aa7c17764dfb791d`.

After switching range responses to direct asynchronous cancellation, the final release build retained the established `Gf-fCJ6TkRU` checksum with a 1.229-second first frame and completed a current FixupX startup probe in 0.356 seconds. Final executable SHA-256: `a9eaa8f1f3f9680988773efa48510fc857bc9fb530666caeacfbf6ffd3368498`.

## Remaining coverage limits

No adaptive quality switching, hardware decoding, arbitrary codecs, DRM or live-stream support was added. Listening quality, perceptual lip sync, extended real-time playback under sustained bandwidth starvation, and this revision on macOS/Linux remain unverified. Direct-media retries still have finite blocking request timeouts. Supported YouTube inputs retain the 20-minute / 128 MiB compressed-media limits. See [support details](youtube.md).
