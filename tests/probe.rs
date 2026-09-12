#![cfg(feature = "native")]

use std::{fs, io::Cursor, process::Command};

#[test]
fn macroscope_probe_accepts_sparse_video_eof() {
    let source = include_bytes!("fixtures/pattern.mp4");
    let mut reader = mp4::Mp4Reader::read_header(Cursor::new(source), source.len() as u64).unwrap();
    let (track_id, timescale, config) = {
        let track = reader.tracks().values().next().unwrap();
        let avc = track.trak.mdia.minf.stbl.stsd.avc1.as_ref().unwrap();
        let mut config = mp4::TrackConfig::from(mp4::AvcConfig {
            width: track.width(),
            height: track.height(),
            seq_param_set: avc.avcc.sequence_parameter_sets[0].bytes.clone(),
            pic_param_set: avc.avcc.picture_parameter_sets[0].bytes.clone(),
        });
        config.timescale = track.timescale();
        (track.track_id(), track.timescale(), config)
    };
    let path = std::env::temp_dir().join(format!("rust-media-sparse-{}.mp4", std::process::id()));
    let file = fs::File::create(&path).unwrap();
    let mut writer = mp4::Mp4Writer::write_start(
        file,
        &mp4::Mp4Config {
            major_brand: "isom".parse().unwrap(),
            minor_version: 512,
            compatible_brands: vec!["isom".parse().unwrap()],
            timescale,
        },
    )
    .unwrap();
    writer.add_track(&config).unwrap();
    for index in 1..=2 {
        let sample = reader.read_sample(track_id, index).unwrap().unwrap();
        writer
            .write_sample(
                1,
                &mp4::Mp4Sample {
                    start_time: 0,
                    duration: timescale * 3,
                    rendering_offset: sample.rendering_offset,
                    is_sync: sample.is_sync,
                    bytes: sample.bytes,
                },
            )
            .unwrap();
    }
    writer.write_end().unwrap();
    drop(writer);
    let output = Command::new(env!("CARGO_BIN_EXE_rust-media"))
        .arg("--probe-all")
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_file(&path).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
