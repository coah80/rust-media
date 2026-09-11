use rust_media::decode::{MediaVideo, Video};
use std::io::Cursor;

const PATTERN: &[u8] = include_bytes!("fixtures/pattern.mp4");
const FRAGMENTED: &[u8] = include_bytes!("fixtures/fragmented.mp4");

fn digest(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}

#[test]
fn repeated_decode_and_seek_is_deterministic() {
    let mut baseline = Video::new(Cursor::new(PATTERN), PATTERN.len() as u64).unwrap();
    let mut expected = Vec::new();
    while let Some((time, pixels)) = baseline.frame().unwrap() {
        expected.push((time, digest(&pixels.rgba)));
    }
    assert_eq!(expected.len(), 48);

    for cycle in 0..100 {
        let mut video = Video::new(Cursor::new(PATTERN), PATTERN.len() as u64).unwrap();
        if cycle % 5 == 0 {
            let mut actual = Vec::new();
            while let Some((time, pixels)) = video.frame().unwrap() {
                actual.push((time, digest(&pixels.rgba)));
            }
            assert_eq!(actual, expected, "full decode cycle {cycle}");
        } else {
            let target = ((cycle * 37) % 190) as f64 / 100.;
            video.seek(target).unwrap();
            let actual = loop {
                let frame = video.frame().unwrap().unwrap();
                if frame.0 >= target {
                    break (frame.0, digest(&frame.1.rgba));
                }
            };
            let expected = expected.iter().find(|frame| frame.0 >= target).unwrap();
            assert_eq!(actual, *expected, "seek cycle {cycle} at {target}");
        }
    }
}

#[test]
fn repeated_fragmented_decode_is_deterministic() {
    let decode = || {
        let mut video = MediaVideo::fragmented(
            Cursor::new(FRAGMENTED),
            Cursor::new(FRAGMENTED),
            FRAGMENTED.len() as u64,
        )
        .unwrap();
        let mut frames = Vec::new();
        while let Some((time, pixels)) = video.frame().unwrap() {
            frames.push((time, digest(&pixels.rgba)));
        }
        frames
    };
    let expected = decode();
    assert_eq!(expected.len(), 48);
    for cycle in 0..25 {
        assert_eq!(decode(), expected, "fragmented decode cycle {cycle}");
    }
}
