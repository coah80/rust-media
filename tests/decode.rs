use rust_media::decode::Video;
use std::io::Cursor;

const PATTERN: &[u8] = include_bytes!("fixtures/pattern.mp4");

#[test]
fn b_frames_are_delivered_in_presentation_order() {
    let mut video = Video::new(Cursor::new(PATTERN), PATTERN.len() as u64).unwrap();
    let mut count = 0;
    let mut previous = -1.;
    let mut first_pixels = Vec::new();
    let mut changed = false;
    while let Some((time, pixels)) = video.frame().unwrap() {
        assert!(time >= previous);
        assert_eq!((pixels.width, pixels.height), (64, 64));
        assert_eq!(pixels.rgba.len(), 64 * 64 * 4);
        if count == 0 {
            assert!(time.abs() < 0.001);
            first_pixels = pixels.rgba.clone();
        } else {
            changed |= pixels.rgba != first_pixels;
        }
        previous = time;
        count += 1;
    }
    assert_eq!(count, 48);
    assert!(changed);
    assert!((video.duration - 2.).abs() < 0.001);
}

#[test]
fn seeking_rebuilds_reference_frames() {
    let mut full = Video::new(Cursor::new(PATTERN), PATTERN.len() as u64).unwrap();
    let mut expected = None;
    while let Some(frame) = full.frame().unwrap() {
        if frame.0 >= 1.25 {
            expected = Some(frame);
            break;
        }
    }
    let mut seeked = Video::new(Cursor::new(PATTERN), PATTERN.len() as u64).unwrap();
    seeked.seek(1.25).unwrap();
    while let Some((time, pixels)) = seeked.frame().unwrap() {
        if time >= 1.25 {
            let (expected_time, expected_pixels) = expected.unwrap();
            assert_eq!(time, expected_time);
            assert_eq!(pixels.rgba, expected_pixels.rgba);
            return;
        }
    }
    panic!("seek produced no frame");
}

#[test]
fn invalid_files_fail_without_a_frame() {
    for bytes in [b"".as_slice(), b"not a video".as_slice(), &PATTERN[..40]] {
        assert!(Video::new(Cursor::new(bytes), bytes.len() as u64).is_err());
    }
}
