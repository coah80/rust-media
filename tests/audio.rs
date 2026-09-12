use rodio::Source;
use rust_media::{audio::Audio, player::MediaReader};
use std::time::Duration;

fn fixture() -> Audio {
    Audio::new(MediaReader::open("tests/fixtures/audio-video.mp4", Default::default()).unwrap())
        .unwrap()
}

#[test]
fn audio_skips_interleaved_video_packets_without_ending() {
    let mut audio = fixture();
    let rate = audio.sample_rate().get() as usize;
    let channels = audio.channels().get() as usize;
    let count = audio.by_ref().count();
    assert!(count >= 2 * rate * channels);
    assert!(count < (rate * channels * 21) / 10);
    assert!(audio.error.lock().unwrap().is_none());
    audio.try_seek(Duration::from_millis(500)).unwrap();
    assert!(audio.next().is_some());
}

#[test]
fn audio_seek_matches_linear_sample_position() {
    let mut linear = fixture();
    let offset = linear.sample_rate().get() as usize * linear.channels().get() as usize;
    let expected: Vec<_> = linear.by_ref().skip(offset).take(4096).collect();
    let mut seeking = fixture();
    seeking.try_seek(Duration::from_secs(1)).unwrap();
    let actual: Vec<_> = seeking.take(4096).collect();
    assert_eq!(expected.len(), actual.len());
    let error = expected
        .iter()
        .zip(actual)
        .map(|(a, b)| (a - b).abs())
        .sum::<f32>()
        / expected.len() as f32;
    assert!(error < 0.01, "mean sample difference {error}");
}
