use rust_media::{decode::Video, fragment::remux};
use std::io::Cursor;

#[test]
fn fragment_offsets_preserve_every_decoded_frame() {
    let fragmented = include_bytes!("fixtures/fragmented.mp4");
    let bytes = remux(fragmented, &Default::default()).unwrap();
    let original = include_bytes!("fixtures/pattern.mp4");
    let mut expected = Video::new(Cursor::new(original), original.len() as u64).unwrap();
    let mut actual = Video::new(Cursor::new(&bytes), bytes.len() as u64).unwrap();
    let mut count = 0;
    while let Some((_, frame)) = expected.frame().unwrap() {
        let (_, result) = actual.frame().unwrap().unwrap();
        assert_eq!(frame.rgba, result.rgba);
        count += 1;
    }
    assert_eq!(count, 48);
    assert!(actual.frame().unwrap().is_none());
}

#[test]
fn fragments_reject_truncation_and_cancelled_assembly() {
    let bytes = include_bytes!("fixtures/fragmented.mp4");
    for length in [0, 7, 15, bytes.len() - 1] {
        assert!(remux(&bytes[..length], &Default::default()).is_err());
    }
    assert!(remux(bytes, &std::sync::atomic::AtomicBool::new(true)).is_err());
}
