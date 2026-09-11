# YouTube support and limits

The provider adapter plays supported public YouTube clips through signed H.264/AAC media and retains SABR as a fallback. Provider behavior can change. The current adapter uses an anonymous VisionOS client profile; it is not an official YouTube embedding SDK.

## Implemented path

1. Read the public watch page, retain its anonymous cookies in memory, and extract visitor data and the initial player response.
2. Request VisionOS player metadata using the same anonymous session.
3. Select direct H.264 up to 720p30 and AAC. Open both with bounded, allowlisted 512 KiB range requests and begin background prefetching.
4. Read the MP4 segment index, parse each H.264 fragment when playback reaches it, and let the shared cache fill in the background. Decode fragmented AAC with Symphonia. No full-file scan or remux is needed on this path.
5. If the direct client request is unavailable, use the existing SABR protobuf path. Rust SWC preprocessing and Boa handle its URL transformation. The SABR parser reassembles bounded UMP parts, carries contexts and validates redirects.
6. Render native frames and play Rodio audio. Video follows the audio clock. Seek and replay use cached blocks or fetch the missing ranges.

## Earlier Windows validation

For the latest memory and integrity results, see [the benchmark report](benchmark-2026-09-11.md). The timings below describe earlier builds.

- `jNQXAC9IVRw`: complete 18.933-second video at 320x240, with 19.064 seconds of AAC. A full live URL check decoded all 284 ordered frames. The final release probe loaded the clip and decoded its first 60 frames in 4.416 seconds. Both Slint renderers passed pause across frames, paused seek, resume, mute, fullscreen, Escape, resize, EOF, replay and replacement with a local file.
- `Gf-fCJ6TkRU`: the progressive direct-client path reached its first 720p frame in 1.166 seconds with 14.7% buffered. It decoded all 4,159 frames and 15,290,368 audio samples without a full-file scan or remux. The final video timestamp and declared duration were both 173.292 seconds. The full release probe completed in 43.483 seconds on the development machine.
- Temporary live checks sought the fragmented video and AAC independently to 120 seconds and `aqz-KE-bpKQ` video to 600 seconds. Both decoded successfully. The temporary checks were removed; the equivalent indexed-fragment seek regression remains.
- `aqz-KE-bpKQ`: this previously reached SABR protection status 3 after about a minute. It now resolves through the direct client path at 854x480 without entering the failing SABR route. A complete release probe decoded 19,037 frames and 55,973,888 audio samples through 634.567 seconds in 86.822 seconds.
- `LXb3EKWsInQ`: a complete 5:14 probe decoded 9,404 frames and 27,680,768 audio samples through 313.780 seconds in 47.047 seconds. An interrupted Googlevideo block reproduced during the first run; bounded same-block retry fixed it and the complete rerun passed.
- `eRsGyueVLvQ`: a complete 14:48 probe decoded 21,313 frames and 78,329,856 audio samples through 888.042 seconds in 177.935 seconds.
- The public FixupX fixture passed the same native controls with the new audio source. The source fixes an earlier bug that could end AAC playback when an interleaved video packet arrived.
- Synthetic fixtures preserve all 48 frames through direct fragmented decoding and fragment assembly, prove indexed startup does not need later segments, compare fragmented seeks with linear decoding, verify cache gaps, decode complete audio and compare seeked PCM with linear decoding. Protocol tests check malformed lengths and timestamp overflow. Runtime tests check cancellation, deadlines and absence of host I/O APIs.

One sequential release run started a new process for each public video:

| Video | Duration | First frame | Buffered at first frame | First 60 frames |
| --- | ---: | ---: | ---: | ---: |
| `Gf-fCJ6TkRU` | 2:53 | 1.647 s | 17.6% | 2.311 s |
| `dQw4w9WgXcQ` | 3:33 | 1.758 s | 11.9% | 2.368 s |
| `LXb3EKWsInQ` | 5:14 | 1.201 s | 7.2% | 1.453 s |
| `aqz-KE-bpKQ` | 10:35 | 1.174 s | 1.9% | 1.387 s |
| `Y-rmzh0PI3c` | 12:11 | 1.498 s | 4.2% | 2.022 s |
| `R6MlUcmOul8` | 12:14 | 1.430 s | 3.2% | 1.838 s |
| `eRsGyueVLvQ` | 14:48 | 1.313 s | 2.7% | 1.733 s |

These timings include public page resolution, player metadata, initial media ranges and software decoding on the development machine. CDN, connection and machine load can change them. `M7lc1UVf-VE` was also checked as a 22:24 boundary case and returned the intended under-20-minute error.

Automated controls checks exercised the active audio output pipeline at zero volume. Listening quality and perceptual lip sync have not been independently assessed. A macOS arm64 progressive release build loaded `Gf-fCJ6TkRU` in the native window and displayed its media title. Updated macOS controls and Linux execution remain unverified.

## Remaining constraints

- `BaW_jenozKc` was unavailable in its public player response. It is not counted as a playback success.
- Direct media starts before complete download and is retained in a shared gap-aware cache. Indexed MP4 fragments load on demand, and interrupted direct-media blocks get two bounded retries. HTTP 4xx responses are not retried. Limits are 128 MiB of combined direct media, 20 minutes of declared duration, 8 MiB per SABR part/segment, 256 SABR requests and a 180-second SABR loading deadline. SABR fallback clips still load completely before playback. These are resource bounds, not availability promises. Peak memory also includes container metadata, decoded frames and Boa.
- One script worker runs at a time. Cancellation releases playback's wait; bounded native parsing may finish before the old worker observes cancellation. Replacing a cancelled YouTube load with a local file took 0.020 seconds in the final cancellation check. Boa execution yields for cancellation checks and has loop, recursion and execution-time limits. Scripts have no host filesystem, process or network APIs. This is not an OS sandbox or a hard allocator limit.
- No account sign-in, proof-of-origin generation, DRM, live streams or adaptive quality switching is implemented. The direct client profile and hardcoded version can change upstream. No partial clip is silently substituted.

## References

- [Googlevideo SABR transport and protocol definitions](https://github.com/LuanRT/googlevideo)
- [Rust SABR audio reference](https://github.com/mthwJsmith/sabr-rs)
- [yt-dlp YouTube client definitions](https://github.com/yt-dlp/yt-dlp/blob/master/yt_dlp/extractor/youtube/_base.py), used to compare current public client profiles and proof-token requirements.
- [Rust script preprocessing](https://github.com/ahaoboy/ytdlp-ejs), pinned to `03399fa26ee36c823b9fb4fc0125381e9e968733`. Only its Rust preprocessing API is called; no downloader CLI or external-runtime feature is enabled.
- [Boa](https://github.com/boa-dev/boa), the Rust JavaScript interpreter.
- [RustyPipe Botguard](https://codeberg.org/ThetaDev/rustypipe-botguard), inspected for its Deno/V8 dependency, not integrated.

Provider behavior can change. These results describe the tested videos and machine, not guaranteed access to other videos or future responses.
