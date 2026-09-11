use std::{
    io::Cursor,
    sync::atomic::{AtomicBool, Ordering},
};

pub fn remux(data: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    remux_inner(data, cancel).map_err(|_| "Could not assemble the video segments".into())
}

fn remux_inner(data: &[u8], cancel: &AtomicBool) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    if cancel.load(Ordering::Relaxed) || data.len() > 128 * 1024 * 1024 {
        return Err("assembly cancelled or too large".into());
    }
    let mut offset = 0usize;
    let mut box_count = 0usize;
    let mut moofs = Vec::new();
    let mut mdats = Vec::new();
    while offset < data.len() {
        box_count += 1;
        if box_count > 100_000 {
            return Err("too many top-level boxes".into());
        }
        let header = data.get(offset..offset + 8).ok_or("short box")?;
        let short_size = u32::from_be_bytes(header[..4].try_into()?);
        let (size, header_size) = if short_size == 1 {
            let extended = data.get(offset + 8..offset + 16).ok_or("short box")?;
            (
                usize::try_from(u64::from_be_bytes(extended.try_into()?))?,
                16,
            )
        } else if short_size == 0 {
            (data.len() - offset, 8)
        } else {
            (short_size as usize, 8)
        };
        if size < header_size || size > data.len() - offset {
            return Err("invalid box".into());
        }
        if &header[4..8] == b"moof" {
            let mut reader = Cursor::new(data);
            crate::decode::validate_fragment_runs(
                &mut reader,
                (offset + header_size) as u64,
                (offset + size) as u64,
            )?;
            moofs.push(offset);
        }
        if &header[4..8] == b"mdat" {
            mdats.push(offset + header_size..offset + size);
        }
        offset += size;
    }
    let reader = mp4::Mp4Reader::read_header(Cursor::new(data), data.len() as u64)?;
    if reader.tracks().len() != 1 || moofs.len() != reader.moofs.len() {
        return Err("invalid tracks".into());
    }
    let track = reader.tracks().values().next().ok_or("missing track")?;
    if track.timescale() == 0 {
        return Err("invalid timescale".into());
    }
    let trex = reader
        .moov
        .mvex
        .as_ref()
        .map(|mvex| &mvex.trex)
        .filter(|trex| trex.track_id == track.track_id());
    let mut config = if let Some(avc) = &track.trak.mdia.minf.stbl.stsd.avc1 {
        if avc.avcc.length_size_minus_one != 3
            || avc.avcc.sequence_parameter_sets.len() != 1
            || avc.avcc.picture_parameter_sets.len() != 1
        {
            return Err("unsupported NAL length".into());
        }
        mp4::TrackConfig::from(mp4::AvcConfig {
            width: track.width(),
            height: track.height(),
            seq_param_set: avc
                .avcc
                .sequence_parameter_sets
                .first()
                .ok_or("missing SPS")?
                .bytes
                .clone(),
            pic_param_set: avc
                .avcc
                .picture_parameter_sets
                .first()
                .ok_or("missing PPS")?
                .bytes
                .clone(),
        })
    } else {
        return Err("fragment assembly requires H.264".into());
    };
    config.timescale = track.timescale();
    let mut writer = mp4::Mp4Writer::write_start(
        Cursor::new(Vec::new()),
        &mp4::Mp4Config {
            major_brand: "isom".parse()?,
            minor_version: 512,
            compatible_brands: vec!["isom".parse()?],
            timescale: track.timescale(),
        },
    )?;
    writer.add_track(&config)?;
    let mut time = 0u64;
    let mut origin = None;
    let mut sample_count = 0usize;
    for (moof, base) in reader.moofs.iter().zip(moofs) {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if moof.trafs.len() != 1 {
            return Err("invalid fragment".into());
        }
        let traf = &moof.trafs[0];
        let run = traf.trun.as_ref().ok_or("missing run")?;
        if traf.tfhd.track_id != track.track_id() || run.sample_count > 100_000 {
            return Err("invalid samples".into());
        }
        let timeline_origin = *origin.get_or_insert_with(|| {
            traf.tfdt
                .as_ref()
                .map(|tfdt| tfdt.base_media_decode_time)
                .unwrap_or(0)
        });
        if let Some(tfdt) = &traf.tfdt
            && tfdt.base_media_decode_time
                != timeline_origin
                    .checked_add(time)
                    .ok_or("timestamp overflow")?
        {
            return Err("noncontiguous fragments".into());
        }
        let base = traf.tfhd.base_data_offset.unwrap_or(base as u64);
        let mut position = base
            .checked_add_signed(i64::from(run.data_offset.ok_or("missing data offset")?))
            .ok_or("invalid offset")? as usize;
        for index in 0..run.sample_count as usize {
            sample_count += 1;
            if sample_count > 1_000_000 {
                return Err("too many samples".into());
            }
            let duration = fragment_value(
                &run.sample_durations,
                index,
                traf.tfhd.default_sample_duration,
                trex.map(|trex| trex.default_sample_duration),
            )
            .ok_or("no duration")?;
            let size = fragment_value(
                &run.sample_sizes,
                index,
                traf.tfhd.default_sample_size,
                trex.map(|trex| trex.default_sample_size),
            )
            .ok_or("no size")? as usize;
            let end = position.checked_add(size).ok_or("overflow")?;
            if size > 8 * 1024 * 1024
                || duration == 0
                || !mdats
                    .iter()
                    .any(|range| range.start <= position && end <= range.end)
            {
                return Err("sample outside media".into());
            }
            let flags = fragment_value(
                &run.sample_flags,
                index,
                if index == 0 {
                    run.first_sample_flags
                } else {
                    None
                }
                .or(traf.tfhd.default_sample_flags),
                trex.map(|trex| trex.default_sample_flags),
            )
            .unwrap_or(0);
            let cts = run.sample_cts.get(index).copied().unwrap_or(0);
            if run.version == 0 && cts > i32::MAX as u32 {
                return Err("invalid composition time".into());
            }
            writer.write_sample(
                1,
                &mp4::Mp4Sample {
                    start_time: time,
                    duration,
                    rendering_offset: cts as i32,
                    is_sync: flags & 0x10000 == 0,
                    bytes: data[position..end].to_vec().into(),
                },
            )?;
            position = end;
            time = time
                .checked_add(u64::from(duration))
                .ok_or("timestamp overflow")?;
        }
    }
    if sample_count == 0 {
        return Err("empty stream".into());
    }
    writer.write_end()?;
    Ok(writer.into_writer().into_inner())
}

fn fragment_value<T: Copy>(
    values: &[T],
    index: usize,
    fragment_default: Option<T>,
    track_default: Option<T>,
) -> Option<T> {
    values
        .get(index)
        .copied()
        .or(fragment_default)
        .or(track_default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macroscope_rejects_excessive_top_level_boxes() {
        let mut data = Vec::with_capacity(800_008);
        for _ in 0..100_001 {
            data.extend_from_slice(&8u32.to_be_bytes());
            data.extend_from_slice(b"mdat");
        }
        assert_eq!(
            remux_inner(&data, &AtomicBool::new(false))
                .unwrap_err()
                .to_string(),
            "too many top-level boxes"
        );
    }

    #[test]
    fn macroscope_remux_uses_track_fragment_defaults() {
        assert_eq!(fragment_value::<u32>(&[], 0, None, Some(7)), Some(7));
        assert_eq!(fragment_value(&[1], 0, Some(2), Some(3)), Some(1));
        assert_eq!(fragment_value::<u32>(&[], 0, Some(2), Some(3)), Some(2));
    }
}
