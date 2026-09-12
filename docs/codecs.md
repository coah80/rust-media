# Codec support

`MediaVideo::open` detects the container from its header. Local files and public HTTPS media URLs use the same decoders. The filename extension is not used to choose a codec.

| Container | Video | Audio |
| --- | --- | --- |
| Ordinary MP4/MOV | H.264, VP9, AV1, HEVC | AAC |
| Fragmented MP4 | H.264 | AAC |
| WebM | VP9, AV1 | Opus, Vorbis |
| Matroska | VP9, AV1, HEVC | Opus, Vorbis |

The Matroska route is implemented, but the recorded decode checks below cover WebM and MP4. AAC inside Matroska, Opus inside MP4, VP8, encrypted tracks, and compressed Matroska tracks are unsupported. Provider adapters still select their existing H.264 renditions. Adding a decoder does not add webpage extraction for a new website.

## Loading and playback

The VP9, AV1, HEVC, and WebM path buffers the compressed file before decoding. It accepts at most 32 MiB per file and keeps selected packets in memory for seeking. Demuxing can temporarily hold both the file and extracted packets. Audio and video use separate readers. This is not a 32 MiB total process-memory guarantee.

Existing H.264 range streaming is unchanged. The new path needs incremental container reading before it can handle long videos with the same startup and memory behavior.

Input dimensions may not exceed 1920 pixels on either side. Output is resized to fit 1280 × 720. Packets are capped at 8 MiB. Container metadata, element counts, nesting, sample counts, timestamps, and decoded plane sizes have separate limits. Unknown-sized Matroska elements other than the outer segment are rejected.

VP9 supports fixed dimensions. AV1 and HEVC output currently requires 4:2:0 chroma. Checks cover 8-bit and 10-bit video. Ten-bit samples are reduced to eight bits for the existing limited-range BT.601 conversion. HDR, tone mapping, and color management are not implemented.

The AV1 decoder has assembly disabled and uses one decoding thread. It can fall behind real time even at 640 × 360. Playback checks catch up in bounded batches so a slow decoder does not indefinitely postpone controls, but an individual decode call still delays them.

Seeking recreates the decoder and starts from a preceding keyframe. MP4 presentation times include composition offsets and the supported edit offset, which matters for HEVC B-frames. Opus and Vorbis seek with preroll; Opus uses packet-specific durations and Matroska codec delay. Final discard padding and sample-exact audio trimming remain unverified.

## Windows checks, September 12, 2026

Generated two-second clips contained 24 frames at 160 × 90. Every output pixel matched an independent FFmpeg decode after applying the same integer YUV-to-RGB conversion. Each clip also decoded after seeking to one second.

- VP9 and AV1 in WebM, each at 8 and 10 bits
- HEVC in MP4 at 8 and 10 bits, including reordered frames
- VP9 and AV1 in MP4 at 8 bits

WebM with Opus and Vorbis decoded audio at 48 kHz, then decoded the remaining audio after a one-second seek. The check verified sample counts and error state, not listening quality or exact synchronization.

The actual `Player` API ran five-second, 640 × 360 clips at 24 fps with muted audio. Loading, pause, seek to three seconds, resume, and stop passed for VP9/Opus, AV1/Opus, and HEVC/AAC. Development-build first-frame times were about 51 ms, 370 ms, and 41 ms respectively. Paused seeks took about 20 ms, 390 ms, and 31 ms. These are local generated files, not network benchmarks or guarantees.

FFmpeg was used only to generate fixtures and reference pixels. The application decoded in Rust. Temporary codec checks and media were removed after validation. Permanent tests cover malformed input, resource limits, and the vendored HEVC bounds fix. macOS and Linux runtime behavior, long files, HDR, and listening quality remain unverified.
