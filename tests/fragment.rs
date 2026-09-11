use rust_media::{
    decode::{MediaVideo, Video},
    fragment::remux,
};
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
fn fragmented_video_decodes_without_full_remux() {
    let fragmented = include_bytes!("fixtures/fragmented.mp4");
    let mut video = MediaVideo::fragmented(
        Cursor::new(fragmented),
        Cursor::new(fragmented),
        fragmented.len() as u64,
    )
    .unwrap();
    let mut count = 0;
    while video.frame().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 48);
}

#[test]
fn fragmented_video_seek_matches_linear_decode() {
    let fragmented = include_bytes!("fixtures/fragmented.mp4");
    let open = || {
        MediaVideo::fragmented(
            Cursor::new(fragmented),
            Cursor::new(fragmented),
            fragmented.len() as u64,
        )
        .unwrap()
    };
    let target = 1.;
    let mut linear = open();
    let expected = loop {
        let frame = linear.frame().unwrap().unwrap();
        if frame.0 >= target {
            break frame;
        }
    };
    let mut seeked = open();
    seeked.seek(target).unwrap();
    let actual = loop {
        let frame = seeked.frame().unwrap().unwrap();
        if frame.0 >= target {
            break frame;
        }
    };
    assert_eq!(expected.0, actual.0);
    assert_eq!(expected.1.rgba, actual.1.rgba);
}

#[test]
fn fragments_reject_truncation_and_cancelled_assembly() {
    let bytes = include_bytes!("fixtures/fragmented.mp4");
    for length in [0, 7, 15, bytes.len() - 1] {
        assert!(remux(&bytes[..length], &Default::default()).is_err());
    }
    assert!(remux(bytes, &std::sync::atomic::AtomicBool::new(true)).is_err());
}
