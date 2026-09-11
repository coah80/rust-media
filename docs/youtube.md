# YouTube transport experiment

Two public videos were checked from this Windows machine: `jNQXAC9IVRw` and
`aqz-KE-bpKQ`. This was an anonymous request experiment, without account cookies.
Neither video achieved end-to-end playback.

## Observations

- Both public watch pages reported playability `OK`.
- The final checks returned 14 and 40 adaptive formats respectively, with zero
  direct media URLs and zero signature-cipher fields.
- Both contained `serverAbrStreamingUrl`. The existing MP4 range reader cannot
  consume that protocol.
- One earlier response for the first video contained a direct itag 18 URL.
  That transient response was not validated through decoding and is not evidence
  of playback support. Subsequent requests did not return it.
- Experimental iOS and Android player requests using the client descriptions
  in the reference implementation both returned HTTP 400 `FAILED_PRECONDITION`.
  This does not establish that all native clients fail; those request variants
  failed here. No authentication or challenge bypass was attempted.

The original adapter error attributed every missing stream to JavaScript
resolution. That was incorrect. The adapter now distinguishes direct but
unsupported formats, signature-cipher metadata, SABR-only metadata and missing
media data. These messages describe the response, not a universal provider rule.

## What implementation would require

SABR uses structured requests and chunked media/control responses, rather than
ordinary byte ranges over an MP4 file. A working integration needs bounded
protocol parsing, stream initialization, audio/video segment assembly, timing,
seek cancellation and buffering. It also needs a successfully validated request
path before it can be called a player feature.

The public `sabr-rs` experiment was inspected as a reference. It targets audio
streaming and is not a verified replacement for this project's video reader.
Its code was not added as a dependency. A Rust JavaScript interpreter could
evaluate provider scripts without a WebView, but would add script execution and
would not by itself implement SABR. No interpreter was added.

The next useful acceptance test is decoding synchronized audio and video from
one public SABR response, followed by pause, seek, cancellation and replay in
the native player. These remain open. The existing local MP4 and FixupX paths
remain the supported experiment.

## Primary references

- [YouTube web client SABR-only responses](https://github.com/yt-dlp/yt-dlp/issues/12482)
- [yt-dlp JavaScript requirements](https://github.com/yt-dlp/yt-dlp/wiki/EJS)
- [Rust SABR audio experiment](https://github.com/mthwJsmith/sabr-rs)
- [Googlevideo protocol definitions](https://github.com/LuanRT/googlevideo)
- [Rust EJS implementation with an optional Boa engine](https://github.com/ahaoboy/ytdlp-ejs)

Provider behavior can change. These observations describe this experiment,
not guaranteed availability for other videos, machines or future requests.
