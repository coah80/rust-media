# YouTube support and limits

The native Rust path plays one complete public YouTube clip with audio, without a browser or external downloader. General YouTube playback is still incomplete because the longer tested video requires client verification.

## Implemented path

1. Read public watch-page metadata without cookies or credentials.
2. Select H.264 up to 720p30 and AAC.
3. Fetch the referenced player script from YouTube. Rust SWC preprocessing and Boa evaluate the URL transformation. This adds JavaScript execution to the original prototype, but no browser or WebView.
4. Send SABR protobuf requests and parse bounded UMP parts. Reassemble interleaved segments, report contiguous buffered ranges, handle context updates and validate redirects.
5. Assemble video fragments into a seekable MP4 without re-encoding. Keep AAC in its original fragmented container and decode it with Symphonia.
6. Render native frames and play Rodio audio. Video follows the audio clock. Seek and replay use loaded media without new network requests.

## Verified on Windows

- `jNQXAC9IVRw`: complete 18.933-second video at 320x240, with 19.064 seconds of AAC. A full live URL check decoded all 284 ordered frames. The final release probe loaded the clip and decoded its first 60 frames in 4.416 seconds. Both Slint renderers passed pause across frames, paused seek, resume, mute, fullscreen, Escape, resize, EOF, replay and replacement with a local file.
- The public FixupX fixture passed the same native controls with the new audio source. The source fixes an earlier bug that could end AAC playback when an interleaved video packet arrived.
- Synthetic fixtures preserve all 48 frames through fragment assembly, decode complete audio and compare seeked PCM with linear decoding. Protocol tests check malformed lengths and timestamp overflow. Runtime tests check cancellation, deadlines and absence of host I/O APIs. These remain as data-integrity and resource-limit regressions; temporary live/UI drivers are removed.

Automated controls checks exercised the active audio output pipeline at zero volume. Listening quality and perceptual lip sync have not been independently assessed. macOS and Linux execution remain unverified.

## Remaining constraints

- `aqz-KE-bpKQ`: repeated SABR requests delivered about 62 seconds of video and 70 seconds of audio, then protection status 3 stopped delivery. The adapter now reports client verification immediately and discards the incomplete clip. Repeated requests did not solve the requirement.
- `BaW_jenozKc` was unavailable in its public player response. It is not counted as a playback success.
- Retested the final Windows release through Mullvad in San Jose on 2026-09-11. The longer video still returned client verification. The short control clip still decoded 60 frames, with the same checksum, in 5.720 seconds. Changing this network route did not remove the longer-video requirement.
- The inspected `rustypipe-botguard` integration uses Deno/V8 and a simulated browser environment. It was not added. Compatible client verification remains an open prerequisite for broader YouTube support.
- Clips load completely before playback. Limits are 128 MiB of received media, 20 minutes of declared duration, 8 MiB per UMP part/segment, 256 requests and a 180-second media-loading deadline. These are resource bounds, not availability promises. Peak memory also includes containers, frames and Boa.
- One script worker runs at a time. Cancellation releases playback's wait; bounded native parsing may finish before the old worker observes cancellation. Replacing a cancelled YouTube load with a local file took 0.020 seconds in the final cancellation check. Boa execution yields for cancellation checks and has loop, recursion and execution-time limits. Scripts have no host filesystem, process or network APIs. This is not an OS sandbox or a hard allocator limit.
- No account sign-in, proof-of-origin generation, DRM, live streams or adaptive quality switching is implemented. No partial clip is silently substituted.

## References

- [Googlevideo SABR transport and protocol definitions](https://github.com/LuanRT/googlevideo)
- [Rust SABR audio reference](https://github.com/mthwJsmith/sabr-rs)
- [Rust script preprocessing](https://github.com/ahaoboy/ytdlp-ejs), pinned to `03399fa26ee36c823b9fb4fc0125381e9e968733`. Only its Rust preprocessing API is called; no downloader CLI or external-runtime feature is enabled.
- [Boa](https://github.com/boa-dev/boa), the Rust JavaScript interpreter.
- [RustyPipe Botguard](https://codeberg.org/ThetaDev/rustypipe-botguard), inspected for its Deno/V8 dependency, not integrated.

Provider behavior can change. These results describe the tested videos and machine, not guaranteed access to other videos or future responses.
