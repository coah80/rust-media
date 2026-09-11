use rust_media::{
    decode::{MediaVideo, Video},
    fragment::remux,
};
use std::{
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::Arc,
};

struct LimitedCursor {
    cursor: Cursor<Arc<[u8]>>,
    limit: u64,
}

struct FragmentedReads {
    cursor: Cursor<Arc<[u8]>>,
    interrupt: Arc<std::sync::atomic::AtomicBool>,
}

impl Read for FragmentedReads {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.interrupt.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let length = output.len().min(97);
        std::thread::sleep(std::time::Duration::from_micros(100));
        self.cursor.read(&mut output[..length])
    }
}

impl Seek for FragmentedReads {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(position)
    }
}

impl LimitedCursor {
    fn new(bytes: Arc<[u8]>, limit: usize) -> Self {
        Self {
            cursor: Cursor::new(bytes),
            limit: limit as u64,
        }
    }
}

impl Read for LimitedCursor {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let position = self.cursor.position();
        if position >= self.limit {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let available = (self.limit - position) as usize;
        let length = output.len().min(available);
        self.cursor.read(&mut output[..length])
    }
}

impl Seek for LimitedCursor {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(position)
    }
}

fn indexed_fixture() -> (Arc<[u8]>, usize) {
    indexed_fixture_with_origin(0)
}

fn indexed_fixture_with_origin(origin: u32) -> (Arc<[u8]>, usize) {
    let source = include_bytes!("fixtures/fragmented.mp4");
    let parsed = mp4::Mp4Reader::read_header(Cursor::new(source), source.len() as u64).unwrap();
    let track = parsed.tracks().values().next().unwrap();
    let track_id = track.track_id();
    let trex = parsed
        .moov
        .mvex
        .as_ref()
        .map(|mvex| &mvex.trex)
        .filter(|trex| trex.track_id == track_id);
    let mut offset = 0usize;
    let mut ranges = Vec::new();
    let mut fragment_start = None;
    while offset < source.len() {
        let size = u32::from_be_bytes(source[offset..offset + 4].try_into().unwrap()) as usize;
        let kind = &source[offset + 4..offset + 8];
        if kind == b"moof" {
            fragment_start = Some(offset);
        } else if kind == b"mdat" {
            ranges.push(fragment_start.take().unwrap()..offset + size);
        }
        offset += size;
    }
    assert_eq!(ranges.len(), parsed.moofs.len());
    let fragments: Vec<(u32, bool)> = parsed
        .moofs
        .iter()
        .map(|moof| {
            let traf = moof
                .trafs
                .iter()
                .find(|traf| traf.tfhd.track_id == track_id)
                .unwrap();
            let run = traf.trun.as_ref().unwrap();
            let duration = (0..run.sample_count as usize)
                .map(|index| {
                    run.sample_durations
                        .get(index)
                        .copied()
                        .or(traf.tfhd.default_sample_duration)
                        .or_else(|| trex.map(|trex| trex.default_sample_duration))
                        .unwrap()
                })
                .sum();
            let flags = run
                .sample_flags
                .first()
                .copied()
                .or(run.first_sample_flags)
                .or(traf.tfhd.default_sample_flags)
                .or_else(|| trex.map(|trex| trex.default_sample_flags))
                .unwrap_or(0);
            (duration, flags & 0x10000 == 0)
        })
        .collect();
    let index_size = 32 + ranges.len() * 12;
    let mut sidx = Vec::with_capacity(index_size);
    sidx.extend_from_slice(&(index_size as u32).to_be_bytes());
    sidx.extend_from_slice(b"sidx");
    sidx.extend_from_slice(&[0; 4]);
    sidx.extend_from_slice(&track_id.to_be_bytes());
    sidx.extend_from_slice(&track.timescale().to_be_bytes());
    sidx.extend_from_slice(&origin.to_be_bytes());
    sidx.extend_from_slice(&0u32.to_be_bytes());
    sidx.extend_from_slice(&0u16.to_be_bytes());
    sidx.extend_from_slice(&(ranges.len() as u16).to_be_bytes());
    for (range, (duration, starts_with_sap)) in ranges.iter().zip(fragments) {
        sidx.extend_from_slice(&((range.end - range.start) as u32).to_be_bytes());
        sidx.extend_from_slice(&duration.to_be_bytes());
        let sap = if starts_with_sap { 0x9000_0000u32 } else { 0 };
        sidx.extend_from_slice(&sap.to_be_bytes());
    }
    let start = ranges[0].start;
    let first_end = start + sidx.len() + ranges[0].len();
    let mut bytes = Vec::with_capacity(source.len() + sidx.len());
    bytes.extend_from_slice(&source[..start]);
    bytes.extend_from_slice(&sidx);
    bytes.extend_from_slice(&source[start..]);
    shift_fragment_times(&mut bytes, u64::from(origin));
    (bytes.into(), first_end)
}

fn shift_fragment_times(bytes: &mut [u8], shift: u64) {
    let mut cursor = 0usize;
    while let Some(relative) = bytes[cursor..]
        .windows(4)
        .position(|value| value == b"tfdt")
    {
        let kind = cursor + relative;
        let version = bytes[kind + 4];
        if version == 0 {
            let start = kind + 8;
            let value = u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap());
            bytes[start..start + 4].copy_from_slice(
                &(u64::from(value) + shift)
                    .try_into()
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
        } else {
            let start = kind + 8;
            let value = u64::from_be_bytes(bytes[start..start + 8].try_into().unwrap());
            bytes[start..start + 8].copy_from_slice(&(value + shift).to_be_bytes());
        }
        cursor = kind + 4;
    }
}

fn add_fragment_edit(bytes: &[u8], media_time: u32, segment_duration: u32) -> Vec<u8> {
    let mut output = bytes.to_vec();
    shift_fragment_times(&mut output, u64::from(media_time));
    let trak_kind = output
        .windows(4)
        .position(|value| value == b"trak")
        .unwrap();
    let trak_start = trak_kind - 4;
    let trak_size = u32::from_be_bytes(output[trak_start..trak_start + 4].try_into().unwrap());
    let moov_kind = output[..trak_start]
        .windows(4)
        .rposition(|value| value == b"moov")
        .unwrap();
    let moov_start = moov_kind - 4;
    let moov_size = u32::from_be_bytes(output[moov_start..moov_start + 4].try_into().unwrap());
    let mut edit = Vec::new();
    edit.extend_from_slice(&36u32.to_be_bytes());
    edit.extend_from_slice(b"edts");
    edit.extend_from_slice(&28u32.to_be_bytes());
    edit.extend_from_slice(b"elst");
    edit.extend_from_slice(&[0; 4]);
    edit.extend_from_slice(&1u32.to_be_bytes());
    edit.extend_from_slice(&segment_duration.to_be_bytes());
    edit.extend_from_slice(&media_time.to_be_bytes());
    edit.extend_from_slice(&1u16.to_be_bytes());
    edit.extend_from_slice(&0u16.to_be_bytes());
    let insert = trak_start + trak_size as usize;
    output.splice(insert..insert, edit);
    output[trak_start..trak_start + 4].copy_from_slice(&(trak_size + 36).to_be_bytes());
    output[moov_start..moov_start + 4].copy_from_slice(&(moov_size + 36).to_be_bytes());
    output
}

fn duplicate_first_trun(bytes: &[u8]) -> Vec<u8> {
    let mut output = bytes.to_vec();
    let trun_kind = output
        .windows(4)
        .position(|value| value == b"trun")
        .unwrap();
    let trun_start = trun_kind - 4;
    let trun_size = u32::from_be_bytes(output[trun_start..trun_start + 4].try_into().unwrap());
    let traf_kind = output[..trun_start]
        .windows(4)
        .rposition(|value| value == b"traf")
        .unwrap();
    let traf_start = traf_kind - 4;
    let traf_size = u32::from_be_bytes(output[traf_start..traf_start + 4].try_into().unwrap());
    let moof_kind = output[..traf_start]
        .windows(4)
        .rposition(|value| value == b"moof")
        .unwrap();
    let moof_start = moof_kind - 4;
    let moof_size = u32::from_be_bytes(output[moof_start..moof_start + 4].try_into().unwrap());
    let duplicate = output[trun_start..trun_start + trun_size as usize].to_vec();
    let insert = traf_start + traf_size as usize;
    output.splice(insert..insert, duplicate);
    output[traf_start..traf_start + 4].copy_from_slice(&(traf_size + trun_size).to_be_bytes());
    output[moof_start..moof_start + 4].copy_from_slice(&(moof_size + trun_size).to_be_bytes());
    output
}

fn extend_mdat_headers(bytes: &[u8]) -> Vec<u8> {
    let mut source = bytes.to_vec();
    let mut offset = 0usize;
    while offset < source.len() {
        let size = u32::from_be_bytes(source[offset..offset + 4].try_into().unwrap()) as usize;
        if &source[offset + 4..offset + 8] == b"moof" {
            let trun = offset
                + source[offset..offset + size]
                    .windows(4)
                    .position(|value| value == b"trun")
                    .unwrap();
            let flags =
                u32::from_be_bytes([0, source[trun + 5], source[trun + 6], source[trun + 7]]);
            assert_ne!(flags & 1, 0);
            let data_offset = trun + 12;
            let value =
                i32::from_be_bytes(source[data_offset..data_offset + 4].try_into().unwrap());
            source[data_offset..data_offset + 4].copy_from_slice(&(value + 8).to_be_bytes());
        }
        offset += size;
    }
    let mut output = Vec::with_capacity(source.len() + 64);
    offset = 0;
    while offset < source.len() {
        let size = u32::from_be_bytes(source[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset + size;
        if &source[offset + 4..offset + 8] == b"mdat" {
            output.extend_from_slice(&1u32.to_be_bytes());
            output.extend_from_slice(b"mdat");
            output.extend_from_slice(&((size + 8) as u64).to_be_bytes());
            output.extend_from_slice(&source[offset + 8..end]);
        } else {
            output.extend_from_slice(&source[offset..end]);
        }
        offset = end;
    }
    output
}

fn multiplex_eager_fragments(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len() * 2);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let size = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        let end = offset + size;
        if &bytes[offset + 4..offset + 8] == b"moov" {
            let trak_kind = bytes[offset..end]
                .windows(4)
                .position(|value| value == b"trak")
                .map(|position| offset + position)
                .unwrap();
            let trak_start = trak_kind - 4;
            let trak_size =
                u32::from_be_bytes(bytes[trak_start..trak_start + 4].try_into().unwrap()) as usize;
            let mut foreign = bytes[trak_start..trak_start + trak_size].to_vec();
            let tkhd = foreign
                .windows(4)
                .position(|value| value == b"tkhd")
                .unwrap();
            let track_id = if foreign[tkhd + 4] == 0 {
                tkhd + 16
            } else {
                tkhd + 24
            };
            foreign[track_id..track_id + 4].copy_from_slice(&u32::MAX.to_be_bytes());
            let hdlr = foreign
                .windows(4)
                .position(|value| value == b"hdlr")
                .unwrap();
            foreign[hdlr + 12..hdlr + 16].copy_from_slice(b"soun");
            let mut moov = bytes[offset..end].to_vec();
            moov[..4].copy_from_slice(&((size + trak_size) as u32).to_be_bytes());
            moov.extend_from_slice(&foreign);
            output.extend_from_slice(&moov);
            offset = end;
            continue;
        }
        if &bytes[offset + 4..offset + 8] != b"moof" {
            output.extend_from_slice(&bytes[offset..end]);
            offset = end;
            continue;
        }
        let mdat_size = u32::from_be_bytes(bytes[end..end + 4].try_into().unwrap()) as usize;
        assert_eq!(&bytes[end + 4..end + 8], b"mdat");
        let mdat_end = end + mdat_size;
        let traf_kind = bytes[offset..end]
            .windows(4)
            .position(|value| value == b"traf")
            .map(|position| offset + position)
            .unwrap();
        let traf_start = traf_kind - 4;
        let traf_size =
            u32::from_be_bytes(bytes[traf_start..traf_start + 4].try_into().unwrap()) as usize;
        let mut foreign = bytes[traf_start..traf_start + traf_size].to_vec();
        let foreign_tfhd = foreign
            .windows(4)
            .position(|value| value == b"tfhd")
            .unwrap();
        foreign[foreign_tfhd + 8..foreign_tfhd + 12].copy_from_slice(&u32::MAX.to_be_bytes());
        let foreign_trun = foreign
            .windows(4)
            .position(|value| value == b"trun")
            .unwrap();
        let foreign_flags = u32::from_be_bytes([
            0,
            foreign[foreign_trun + 5],
            foreign[foreign_trun + 6],
            foreign[foreign_trun + 7],
        ]);
        assert_ne!(foreign_flags & 1, 0);
        let new_moof_size = size + traf_size;
        foreign[foreign_trun + 12..foreign_trun + 16]
            .copy_from_slice(&((new_moof_size + 8) as i32).to_be_bytes());
        let mut moof = bytes[offset..end].to_vec();
        moof[..4].copy_from_slice(&(new_moof_size as u32).to_be_bytes());
        let local_traf = traf_start - offset;
        let video_tfhd = local_traf
            + moof[local_traf..local_traf + traf_size]
                .windows(4)
                .position(|value| value == b"tfhd")
                .unwrap();
        let video_flags = u32::from_be_bytes([
            0,
            moof[video_tfhd + 5],
            moof[video_tfhd + 6],
            moof[video_tfhd + 7],
        ]);
        assert_eq!(video_flags & 1, 0);
        moof[video_tfhd + 5] &= !2;
        let video_trun = local_traf
            + moof[local_traf..local_traf + traf_size]
                .windows(4)
                .position(|value| value == b"trun")
                .unwrap();
        moof[video_trun + 12..video_trun + 16].copy_from_slice(&0i32.to_be_bytes());
        moof.splice(local_traf..local_traf, foreign);
        let payload = &bytes[end + 8..mdat_end];
        output.extend_from_slice(&moof);
        output.extend_from_slice(&((8 + payload.len() * 2) as u32).to_be_bytes());
        output.extend_from_slice(b"mdat");
        output.extend_from_slice(payload);
        output.extend_from_slice(payload);
        offset = mdat_end;
    }
    output
}

fn prepend_foreign_sidx(bytes: &[u8]) -> Vec<u8> {
    let kind = bytes.windows(4).position(|value| value == b"sidx").unwrap();
    let start = kind - 4;
    let size = u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
    let mut foreign = bytes[start..start + size].to_vec();
    foreign[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
    let mut output = Vec::with_capacity(bytes.len() + size);
    output.extend_from_slice(&bytes[..start]);
    output.extend_from_slice(&foreign);
    output.extend_from_slice(&bytes[start..]);
    output
}

fn split_video_sidx(bytes: &[u8]) -> Vec<u8> {
    let kind = bytes.windows(4).position(|value| value == b"sidx").unwrap();
    let start = kind - 4;
    let size = u32::from_be_bytes(bytes[start..start + 4].try_into().unwrap()) as usize;
    assert_eq!(bytes[start + 8], 0);
    let reference_id = u32::from_be_bytes(bytes[start + 12..start + 16].try_into().unwrap());
    let timescale = u32::from_be_bytes(bytes[start + 16..start + 20].try_into().unwrap());
    let earliest = u32::from_be_bytes(bytes[start + 20..start + 24].try_into().unwrap());
    let first_offset = u32::from_be_bytes(bytes[start + 24..start + 28].try_into().unwrap());
    let count = u16::from_be_bytes(bytes[start + 30..start + 32].try_into().unwrap()) as usize;
    let split = count / 2;
    assert!(split > 0 && split < count);
    let entries = &bytes[start + 32..start + size];
    let first_length: u32 = entries[..split * 12]
        .as_chunks::<12>()
        .0
        .iter()
        .map(|entry| u32::from_be_bytes(entry[..4].try_into().unwrap()) & 0x7fff_ffff)
        .sum();
    let first_duration: u32 = entries[..split * 12]
        .as_chunks::<12>()
        .0
        .iter()
        .map(|entry| u32::from_be_bytes(entry[4..8].try_into().unwrap()))
        .sum();
    let second_size = 32 + (count - split) * 12;
    let index = |entries: &[u8], earliest: u32, first_offset: u32| {
        let mut index = Vec::with_capacity(32 + entries.len());
        index.extend_from_slice(&((32 + entries.len()) as u32).to_be_bytes());
        index.extend_from_slice(b"sidx");
        index.extend_from_slice(&[0; 4]);
        index.extend_from_slice(&reference_id.to_be_bytes());
        index.extend_from_slice(&timescale.to_be_bytes());
        index.extend_from_slice(&earliest.to_be_bytes());
        index.extend_from_slice(&first_offset.to_be_bytes());
        index.extend_from_slice(&0u16.to_be_bytes());
        index.extend_from_slice(&((entries.len() / 12) as u16).to_be_bytes());
        index.extend_from_slice(entries);
        index
    };
    let first = index(
        &entries[..split * 12],
        earliest,
        first_offset + second_size as u32,
    );
    let second = index(
        &entries[split * 12..],
        earliest + first_duration,
        first_offset + first_length,
    );
    let mut output = Vec::with_capacity(bytes.len() + 32);
    output.extend_from_slice(&bytes[..start]);
    output.extend_from_slice(&first);
    output.extend_from_slice(&second);
    output.extend_from_slice(&bytes[start + size..]);
    output
}

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
fn macroscope_eager_fragments_follow_preceding_traf() {
    let source = include_bytes!("fixtures/fragmented.mp4");
    let multiplexed = multiplex_eager_fragments(source);
    let mut expected = MediaVideo::fragmented(
        Cursor::new(source),
        Cursor::new(source),
        source.len() as u64,
    )
    .unwrap();
    let mut actual = MediaVideo::fragmented(
        Cursor::new(&multiplexed),
        Cursor::new(&multiplexed),
        multiplexed.len() as u64,
    )
    .unwrap();
    let mut frames = 0;
    while let Some(expected) = expected.frame().unwrap() {
        let actual = actual.frame().unwrap().unwrap();
        assert_eq!(actual.0, expected.0);
        assert_eq!(actual.1.rgba, expected.1.rgba);
        frames += 1;
    }
    assert_eq!(frames, 48);
    assert!(actual.frame().unwrap().is_none());
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
fn indexed_fragments_start_without_later_segments_and_seek() {
    let (bytes, first_end) = indexed_fixture();
    let mut startup = MediaVideo::fragmented(
        LimitedCursor::new(bytes.clone(), first_end),
        LimitedCursor::new(bytes.clone(), bytes.len()),
        bytes.len() as u64,
    )
    .unwrap();
    assert!(startup.frame().unwrap().is_some());

    let open = || {
        MediaVideo::fragmented(
            Cursor::new(bytes.clone()),
            Cursor::new(bytes.clone()),
            bytes.len() as u64,
        )
        .unwrap()
    };
    let mut complete = open();
    let mut count = 0;
    while complete.frame().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, 48);

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
fn macroscope_multiple_video_indexes_are_merged() {
    let (bytes, _) = indexed_fixture();
    let bytes = split_video_sidx(&bytes);
    let mut video =
        MediaVideo::fragmented(Cursor::new(&bytes), Cursor::new(&bytes), bytes.len() as u64)
            .unwrap();
    assert!((video.duration() - 2.).abs() < 0.001);
    let mut frames = 0;
    while video.frame().unwrap().is_some() {
        frames += 1;
    }
    assert_eq!(frames, 48);
}

#[test]
fn fragments_reject_truncation_and_cancelled_assembly() {
    let bytes = include_bytes!("fixtures/fragmented.mp4");
    for length in [0, 7, 15, bytes.len() - 1] {
        assert!(remux(&bytes[..length], &Default::default()).is_err());
    }
    assert!(remux(bytes, &std::sync::atomic::AtomicBool::new(true)).is_err());
}

#[test]
fn indexed_seeks_preserve_pixels_across_repeated_direction_changes() {
    let (bytes, _) = indexed_fixture();
    let open = || {
        MediaVideo::fragmented(
            Cursor::new(bytes.clone()),
            Cursor::new(bytes.clone()),
            bytes.len() as u64,
        )
        .unwrap()
    };
    let mut linear = open();
    let mut frames = Vec::new();
    while let Some(frame) = linear.frame().unwrap() {
        frames.push(frame);
    }
    let mut seeking = open();
    for step in 0..240 {
        let target = ((step * 37) % 190) as f64 / 100.;
        let expected = frames.iter().find(|frame| frame.0 >= target).unwrap();
        seeking.seek(target).unwrap();
        let actual = loop {
            let frame = seeking.frame().unwrap().unwrap();
            if frame.0 >= target {
                break frame;
            }
        };
        assert_eq!(actual.0, expected.0, "seek {step} at {target}");
        assert_eq!(actual.1.rgba, expected.1.rgba, "seek {step} at {target}");
    }
}

#[test]
fn delayed_short_reads_and_interruption_preserve_frames() {
    let (bytes, _) = indexed_fixture();
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = || FragmentedReads {
        cursor: Cursor::new(bytes.clone()),
        interrupt: interrupted.clone(),
    };
    let mut actual = MediaVideo::fragmented(reader(), reader(), bytes.len() as u64).unwrap();
    let mut expected = MediaVideo::fragmented(
        Cursor::new(bytes.clone()),
        Cursor::new(bytes.clone()),
        bytes.len() as u64,
    )
    .unwrap();
    interrupted.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(actual.frame().is_err());
    interrupted.store(false, std::sync::atomic::Ordering::Relaxed);
    let mut count = 0;
    while let Some(frame) = expected.frame().unwrap() {
        let result = actual.frame().unwrap().unwrap();
        assert_eq!(frame.0, result.0);
        assert_eq!(frame.1.rgba, result.1.rgba);
        count += 1;
    }
    assert_eq!(count, 48);
    assert!(actual.frame().unwrap().is_none());
}

#[test]
fn macroscope_indexed_timeline_starts_at_zero() {
    let source = include_bytes!("fixtures/fragmented.mp4");
    let parsed = mp4::Mp4Reader::read_header(Cursor::new(source), source.len() as u64).unwrap();
    let timescale = parsed.tracks().values().next().unwrap().timescale();
    let (bytes, _) = indexed_fixture_with_origin(timescale * 5);
    let mut video = MediaVideo::fragmented(
        Cursor::new(bytes.clone()),
        Cursor::new(bytes.clone()),
        bytes.len() as u64,
    )
    .unwrap();
    let first = video.frame().unwrap().unwrap();
    let (baseline, _) = indexed_fixture();
    let mut baseline_video = MediaVideo::fragmented(
        Cursor::new(baseline.clone()),
        Cursor::new(baseline.clone()),
        baseline.len() as u64,
    )
    .unwrap();
    let baseline_first = baseline_video.frame().unwrap().unwrap();
    assert!((first.0 - baseline_first.0).abs() < 1e-9);
    assert_eq!(first.1.rgba, baseline_first.1.rgba);
    assert!((video.duration() - 2.).abs() < 0.001);
    video.seek(1.).unwrap();
    let seeked = loop {
        let frame = video.frame().unwrap().unwrap();
        if frame.0 >= 1. {
            break frame;
        }
    };
    let mut expected = MediaVideo::fragmented(
        Cursor::new(baseline.clone()),
        Cursor::new(baseline.clone()),
        baseline.len() as u64,
    )
    .unwrap();
    expected.seek(1.).unwrap();
    let expected = loop {
        let frame = expected.frame().unwrap().unwrap();
        if frame.0 >= 1. {
            break frame;
        }
    };
    assert!((seeked.0 - expected.0).abs() < 1e-9);
    assert_eq!(seeked.1.rgba, expected.1.rgba);
}

#[test]
fn macroscope_fragment_edit_uses_movie_timeline() {
    let source = include_bytes!("fixtures/fragmented.mp4");
    let parsed = mp4::Mp4Reader::read_header(Cursor::new(source), source.len() as u64).unwrap();
    let track_scale = parsed.tracks().values().next().unwrap().timescale();
    let movie_duration = parsed.timescale() * 2;
    let bytes = add_fragment_edit(source, track_scale, movie_duration);
    let mut video = MediaVideo::fragmented(
        Cursor::new(bytes.clone()),
        Cursor::new(bytes.clone()),
        bytes.len() as u64,
    )
    .unwrap();
    let first = video.frame().unwrap().unwrap();
    let mut baseline = MediaVideo::fragmented(
        Cursor::new(source),
        Cursor::new(source),
        source.len() as u64,
    )
    .unwrap();
    let baseline_first = baseline.frame().unwrap().unwrap();
    assert!((first.0 - baseline_first.0).abs() < 1e-9);
    assert_eq!(first.1.rgba, baseline_first.1.rgba);
    assert!((video.duration() - 2.).abs() < 0.001);
}

#[test]
fn macroscope_multiple_fragment_runs_are_rejected() {
    let bytes = duplicate_first_trun(include_bytes!("fixtures/fragmented.mp4"));
    let error = match MediaVideo::fragmented(
        Cursor::new(bytes.clone()),
        Cursor::new(bytes.clone()),
        bytes.len() as u64,
    ) {
        Ok(_) => panic!("multiple runs were accepted"),
        Err(error) => error,
    };
    assert_eq!(error, "Multiple video fragment runs are not supported");
}

#[test]
fn macroscope_remux_normalizes_nonzero_fragment_origin() {
    let source = include_bytes!("fixtures/fragmented.mp4");
    let mut shifted = source.to_vec();
    shift_fragment_times(&mut shifted, 90_000);
    let expected = remux(source, &Default::default()).unwrap();
    let actual = remux(&shifted, &Default::default()).unwrap();
    let mut expected = Video::new(Cursor::new(&expected), expected.len() as u64).unwrap();
    let mut actual = Video::new(Cursor::new(&actual), actual.len() as u64).unwrap();
    while let Some(frame) = expected.frame().unwrap() {
        let result = actual.frame().unwrap().unwrap();
        assert_eq!(result.0, frame.0);
        assert_eq!(result.1.rgba, frame.1.rgba);
    }
    assert!(actual.frame().unwrap().is_none());
}

#[test]
fn macroscope_remux_accepts_extended_mdat() {
    let source = include_bytes!("fixtures/fragmented.mp4");
    let extended = extend_mdat_headers(source);
    let expected = remux(source, &Default::default()).unwrap();
    let actual = remux(&extended, &Default::default()).unwrap();
    let mut expected = Video::new(Cursor::new(&expected), expected.len() as u64).unwrap();
    let mut actual = Video::new(Cursor::new(&actual), actual.len() as u64).unwrap();
    while let Some(frame) = expected.frame().unwrap() {
        let result = actual.frame().unwrap().unwrap();
        assert_eq!(result.0, frame.0);
        assert_eq!(result.1.rgba, frame.1.rgba);
    }
    assert!(actual.frame().unwrap().is_none());
}

#[test]
fn macroscope_indexed_video_skips_foreign_sidx() {
    let (bytes, _) = indexed_fixture();
    let bytes = prepend_foreign_sidx(&bytes);
    let mut video = MediaVideo::fragmented(
        Cursor::new(bytes.clone()),
        Cursor::new(bytes.clone()),
        bytes.len() as u64,
    )
    .unwrap();
    let mut frames = 0;
    while video.frame().unwrap().is_some() {
        frames += 1;
    }
    assert_eq!(frames, 48);
}
