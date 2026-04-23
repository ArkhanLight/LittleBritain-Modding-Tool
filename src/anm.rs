use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct RigidAnimStream {
    pub stream_index: usize,
    pub rotations_xyzw: Vec<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct RigidAnimClip {
    pub sample_rate: f32,
    pub duration_seconds: f32,
    pub frame_times: Vec<f32>,
    pub streams: Vec<RigidAnimStream>,
}

#[derive(Clone, Debug)]
pub struct AnmFile {
    pub path: PathBuf,
    pub file_size: usize,
    pub version_major: u32,
    pub version_minor: u32,
    pub payload_size: u32,
    pub rig_bone_count: usize,
    pub duration_hint_seconds: f32,
    pub section_count_hint: u32,
    pub section_table_offset: u32,
    pub timing_table_offset: u32,
    pub key_block_count_hint: u32,
    pub key_count_hint: u32,
    pub header_words: [u32; 16],
    pub timing_samples: Vec<f32>,
    pub timing_offsets: Vec<u32>,
    pub embedded_strings: Vec<String>,
    pub rigid_clip: Option<RigidAnimClip>,
}

pub fn load_anm(path: &Path) -> Result<AnmFile> {
    let data = fs::read(path)
        .with_context(|| format!("Failed to read ANM file {}", path.display()))?;

    if data.len() < 64 {
        bail!("ANM file is too small to contain the 64-byte header");
    }

    let mut header_words = [0u32; 16];
    for (i, slot) in header_words.iter_mut().enumerate() {
        *slot = read_u32(&data, i * 4)?;
    }

    let duration_hint_seconds = f32::from_bits(header_words[8]);
    let timing_table_offset = header_words[11];
    let timing_samples = parse_full_timing_samples(
        &data,
        header_words[10] as usize,
        timing_table_offset as usize,
        header_words[9] as usize,
        duration_hint_seconds,
    );
    let timing_offsets = parse_tail_offsets(&data, timing_table_offset as usize, duration_hint_seconds);

    let rigid_clip = decode_framed_cursor_clip(
        &data,
        header_words[6] as usize,
        header_words[10] as usize,
        duration_hint_seconds,
        &timing_samples,
        &timing_offsets,
    )
    .or_else(|| {
        decode_experimental_rigid_clip(
            &data,
            header_words[6] as usize,
            header_words[3] as usize,
            header_words[10] as usize,
            duration_hint_seconds,
            &timing_samples,
        )
    });

    let embedded_strings = extract_ascii_strings(&data, 4)
        .into_iter()
        .filter(|s| looks_like_reference_string(s))
        .take(16)
        .collect();    

    Ok(AnmFile {
        path: path.to_path_buf(),
        file_size: data.len(),
        version_major: header_words[1],
        version_minor: header_words[2],
        payload_size: header_words[3],
        rig_bone_count: header_words[6] as usize,
        duration_hint_seconds,
        section_count_hint: header_words[9],
        section_table_offset: header_words[10],
        timing_table_offset,
        key_block_count_hint: header_words[12],
        key_count_hint: header_words[13],
        header_words,
        timing_samples,
        timing_offsets,
        embedded_strings,
        rigid_clip,
    })
}

fn decode_experimental_rigid_clip(
    data: &[u8],
    bone_count: usize,
    payload_size: usize,
    section_table_offset: usize,
    duration_hint_seconds: f32,
    frame_times: &[f32],
) -> Option<RigidAnimClip> {
    if bone_count == 0 || bone_count > 64 || data.len() <= 96 {
        return None;
    }

    let mut body_end = payload_size.min(data.len());
    if section_table_offset > 64 {
        body_end = body_end.min(section_table_offset);
    }

    if body_end <= 64 + 32 {
        return None;
    }

    let target_frame_count = if frame_times.len() >= 2 {
        frame_times.len()
    } else {
        0
    };

    let mut streams: BTreeMap<usize, Vec<[f32; 4]>> = BTreeMap::new();
    let mut current_stream: Option<usize> = None;

    let mut packet_off = 64usize;
    while packet_off + 32 <= body_end {
        let groups = [
            read_i16x4(data, packet_off).ok()?,
            read_i16x4(data, packet_off + 8).ok()?,
            read_i16x4(data, packet_off + 16).ok()?,
            read_i16x4(data, packet_off + 24).ok()?,
        ];

        for group in groups {
            if let Some(stream_index) = decode_metadata_stream_index(group, bone_count) {
                current_stream = Some(stream_index);
                continue;
            }

            if let (Some(stream_index), Some(quat)) = (current_stream, decode_quat_group(group)) {
                streams.entry(stream_index).or_default().push(quat);
            }
        }

        packet_off += 32;
    }

    let mut streams: Vec<(usize, Vec<[f32; 4]>)> = streams
        .into_iter()
        .filter(|(_, samples)| samples.len() >= 2)
        .collect();

    if streams.is_empty() {
        return None;
    }

    streams.sort_by_key(|(stream_index, _)| *stream_index);

    let normalized_frame_count = if target_frame_count >= 2 {
        target_frame_count
    } else {
        streams
            .iter()
            .map(|(_, samples)| samples.len())
            .min()
            .unwrap_or(0)
    };

    if normalized_frame_count < 2 {
        return None;
    }

    for (_, samples) in &mut streams {
        *samples = normalize_quat_track_len(samples, normalized_frame_count);
    }

    let duration_seconds = if let Some(&last) = frame_times.last() {
        last.max(1.0 / 30.0)
    } else if duration_hint_seconds.is_finite() && duration_hint_seconds > 0.0 {
        duration_hint_seconds
    } else {
        ((normalized_frame_count.saturating_sub(1)) as f32 / 30.0).max(1.0 / 30.0)
    };

    let sample_rate = if normalized_frame_count >= 2 && duration_seconds > 0.0 {
        ((normalized_frame_count - 1) as f32 / duration_seconds).max(1.0)
    } else {
        30.0
    };

    let frame_times = if frame_times.len() == normalized_frame_count {
        frame_times.to_vec()
    } else {
        build_uniform_frame_times(normalized_frame_count, duration_seconds)
    };

    Some(RigidAnimClip {
        sample_rate,
        duration_seconds,
        frame_times,
        streams: streams
            .into_iter()
            .map(|(stream_index, rotations_xyzw)| RigidAnimStream {
                stream_index,
                rotations_xyzw,
            })
            .collect(),
    })
}

fn decode_framed_cursor_clip(
    data: &[u8],
    bone_count: usize,
    section_table_offset: usize,
    duration_hint_seconds: f32,
    frame_times: &[f32],
    frame_offsets: &[u32],
) -> Option<RigidAnimClip> {
    if bone_count == 0 || frame_offsets.len() < 2 {
        return None;
    }

    if !looks_like_framed_cursor_layout(data, section_table_offset, frame_offsets) {
        return None;
    }

    let frame_count = if frame_times.len() >= 2 {
        frame_times.len().min(frame_offsets.len())
    } else {
        frame_offsets.len()
    };

    if frame_count < 2 {
        return None;
    }

    let mut records = Vec::with_capacity(frame_count);
    for &off in &frame_offsets[..frame_count] {
        records.push(read_u32x7(data, off as usize).ok()?);
    }

    let mut channel_streams: [Option<usize>; 4] = [None, None, None, None];
    let mut tracks: BTreeMap<usize, Vec<[f32; 4]>> = BTreeMap::new();
    let mut total_updates = 0usize;

    for frame_index in 0..frame_count {
        let updates = if frame_index == 0 {
            BTreeMap::new()
        } else {
            decode_frame_cursor_delta(
                data,
                bone_count,
                section_table_offset,
                &records[frame_index - 1],
                &records[frame_index],
                &mut channel_streams,
            )
        };

        total_updates += updates.len();

        for (stream_index, quat) in updates {
            if let Some(track) = tracks.get_mut(&stream_index) {
                while track.len() < frame_index {
                    let fill = *track.last().unwrap_or(&quat);
                    track.push(fill);
                }
                track.push(quat);
            } else {
                tracks.insert(stream_index, vec![quat; frame_index + 1]);
            }
        }

        for track in tracks.values_mut() {
            if track.len() < frame_index + 1 {
                let fill = *track.last().unwrap();
                track.push(fill);
            }
        }
    }

    if tracks.is_empty() || total_updates < 6 {
        return None;
    }

    let duration_seconds = if frame_times.len() >= frame_count {
        frame_times[frame_count - 1].max(1.0 / 30.0)
    } else if duration_hint_seconds.is_finite() && duration_hint_seconds > 0.0 {
        duration_hint_seconds
    } else {
        ((frame_count.saturating_sub(1)) as f32 / 30.0).max(1.0 / 30.0)
    };

    let sample_rate = if frame_count >= 2 && duration_seconds > 0.0 {
        ((frame_count - 1) as f32 / duration_seconds).max(1.0)
    } else {
        30.0
    };

    let frame_times = if frame_times.len() >= frame_count {
        frame_times[..frame_count].to_vec()
    } else {
        build_uniform_frame_times(frame_count, duration_seconds)
    };

    Some(RigidAnimClip {
        sample_rate,
        duration_seconds,
        frame_times,
        streams: tracks
            .into_iter()
            .map(|(stream_index, rotations_xyzw)| RigidAnimStream {
                stream_index,
                rotations_xyzw,
            })
            .collect(),
    })
}

fn looks_like_framed_cursor_layout(
    data: &[u8],
    section_table_offset: usize,
    frame_offsets: &[u32],
) -> bool {
    if frame_offsets.len() < 2 {
        return false;
    }

    if frame_offsets
        .iter()
        .any(|&off| off as usize + 28 > data.len())
    {
        return false;
    }

    let mut diffs = Vec::new();
    for pair in frame_offsets.windows(2) {
        let a = pair[0] as usize;
        let b = pair[1] as usize;

        if b <= a {
            return false;
        }

        diffs.push(b - a);
    }

    let small_record_count = diffs
        .iter()
        .filter(|&&d| (8..=40).contains(&d))
        .count();

    let first = frame_offsets[0] as usize;
    let last = *frame_offsets.last().unwrap() as usize;

    small_record_count * 2 >= diffs.len()
        && first >= 64
        && last < section_table_offset
}

fn decode_frame_cursor_delta(
    data: &[u8],
    bone_count: usize,
    section_table_offset: usize,
    prev_record: &[u32; 7],
    cur_record: &[u32; 7],
    channel_streams: &mut [Option<usize>; 4],
) -> BTreeMap<usize, [f32; 4]> {
    let mut updates = BTreeMap::new();

    for channel in 0..4 {
        let start = prev_record[channel] as usize;
        let end = cur_record[channel] as usize;

        if !(64 <= start && start < end && end <= section_table_offset) {
            continue;
        }

        let mut current_stream = channel_streams[channel];
        let slice = &data[start..end];
        let usable = slice.len() / 8 * 8;

        for chunk in slice[..usable].chunks_exact(8) {
            let group = [
                i16::from_le_bytes([chunk[0], chunk[1]]),
                i16::from_le_bytes([chunk[2], chunk[3]]),
                i16::from_le_bytes([chunk[4], chunk[5]]),
                i16::from_le_bytes([chunk[6], chunk[7]]),
            ];

            if let Some(stream_index) = decode_metadata_stream_index(group, bone_count) {
                current_stream = Some(stream_index);
                continue;
            }

            if let (Some(stream_index), Some(quat)) =
                (current_stream, decode_quat_group(group))
            {
                updates.insert(stream_index, quat);
            }
        }

        channel_streams[channel] = current_stream;
    }

    updates
}

fn read_u32x7(data: &[u8], off: usize) -> Result<[u32; 7]> {
    Ok([
        read_u32(data, off)?,
        read_u32(data, off + 4)?,
        read_u32(data, off + 8)?,
        read_u32(data, off + 12)?,
        read_u32(data, off + 16)?,
        read_u32(data, off + 20)?,
        read_u32(data, off + 24)?,
    ])
}

fn parse_full_timing_samples(
    data: &[u8],
    section_table_offset: usize,
    timing_table_offset: usize,
    expected_count: usize,
    duration_hint: f32,
) -> Vec<f32> {
    let mut out = Vec::new();
    let max_reasonable = if duration_hint.is_finite() && duration_hint > 0.0 {
        duration_hint.max(0.5) + 1.0
    } else {
        32.0
    };

    if section_table_offset + 16 <= data.len() && timing_table_offset <= data.len() {
        let mut cursor = section_table_offset + 16;
        while cursor + 4 <= timing_table_offset {
            let value = f32::from_bits(u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]));

            if !value.is_finite() || value < 0.0 || value > max_reasonable {
                break;
            }

            if let Some(&last) = out.last() {
                if value + 1.0e-4 < last {
                    break;
                }
            }

            out.push(value);
            cursor += 4;

            if expected_count > 0 && out.len() >= expected_count {
                break;
            }

            if out.len() >= 512 {
                break;
            }
        }
    }

    if timing_table_offset < data.len() {
        let mut cursor = timing_table_offset;
        while cursor + 4 <= data.len() {
            let value = f32::from_bits(u32::from_le_bytes([
                data[cursor],
                data[cursor + 1],
                data[cursor + 2],
                data[cursor + 3],
            ]));

            if !value.is_finite() || value < 0.0 || value > max_reasonable {
                break;
            }

            if let Some(&last) = out.last() {
                if value + 1.0e-4 < last {
                    break;
                }
            }

            out.push(value);
            cursor += 4;

            if expected_count > 0 && out.len() >= expected_count {
                break;
            }

            if out.len() >= 512 {
                break;
            }
        }
    }

    // Character-family clips in this game often omit the explicit 0.0 frame time
    // even though the frame-offset table still includes frame 0.
    if expected_count > 0
        && out.len() + 1 == expected_count
        && out.first().copied().unwrap_or(0.0) > 1.0e-6
    {
        out.insert(0, 0.0);
    }

    out
}

fn parse_tail_offsets(data: &[u8], offset: usize, duration_hint: f32) -> Vec<u32> {
    if offset >= data.len() {
        return Vec::new();
    }

    let max_reasonable = if duration_hint.is_finite() && duration_hint > 0.0 {
        duration_hint.max(0.5) + 1.0
    } else {
        32.0
    };

    let mut cursor = offset;
    while cursor + 4 <= data.len() {
        let value = f32::from_bits(u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]));

        if !value.is_finite() || value < 0.0 || value > max_reasonable {
            break;
        }

        cursor += 4;
        if cursor >= data.len() {
            return Vec::new();
        }
    }

    if cursor + 4 <= data.len() {
        let maybe_zero = u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]);
        if maybe_zero == 0 {
            cursor += 4;
        }
    }

    let mut offsets = Vec::new();
    while cursor + 4 <= data.len() {
        let value = u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]);

        if value == 0 || value == 0xCCCC_CCCC {
            break;
        }

        if value as usize >= data.len() {
            break;
        }

        offsets.push(value);
        cursor += 4;

        if offsets.len() >= 512 {
            break;
        }
    }

    offsets
}

fn normalize_quat_track_len(samples: &[[f32; 4]], target_len: usize) -> Vec<[f32; 4]> {
    if target_len == 0 {
        return Vec::new();
    }

    if samples.is_empty() {
        return vec![[0.0, 0.0, 0.0, 1.0]; target_len];
    }

    if samples.len() == 1 {
        return vec![samples[0]; target_len];
    }

    if samples.len() == target_len {
        return samples.to_vec();
    }

    let src_last = (samples.len() - 1) as f32;
    let dst_last = (target_len - 1) as f32;

    let mut out = Vec::with_capacity(target_len);
    for i in 0..target_len {
        let t = if target_len <= 1 {
            0.0
        } else {
            i as f32 / dst_last
        };

        let src_pos = t * src_last;
        let src_index = src_pos.round() as usize;
        let src_index = src_index.min(samples.len() - 1);
        out.push(samples[src_index]);
    }

    out
}

fn build_uniform_frame_times(frame_count: usize, duration_seconds: f32) -> Vec<f32> {
    if frame_count == 0 {
        return Vec::new();
    }

    if frame_count == 1 {
        return vec![0.0];
    }

    let last = duration_seconds.max(1.0 / 30.0);
    let denom = (frame_count - 1) as f32;

    (0..frame_count)
        .map(|i| last * (i as f32 / denom))
        .collect()
}

fn decode_metadata_stream_index(group: [i16; 4], bone_count: usize) -> Option<usize> {
    let stream_index = group[3];
    if stream_index < 0 || stream_index as usize >= bone_count {
        return None;
    }

    if group[1].abs() <= 64 && group[2].abs() <= 4096 {
        return Some(stream_index as usize);
    }

    None
}

fn decode_quat_group(group: [i16; 4]) -> Option<[f32; 4]> {
    let x = group[0] as f32 / 32767.0;
    let y = group[1] as f32 / 32767.0;
    let z = group[2] as f32 / 32767.0;
    let w = group[3] as f32 / 32767.0;

    let norm = (x * x + y * y + z * z + w * w).sqrt();
    if !(0.90..=1.10).contains(&norm) {
        return None;
    }

    let n = norm.max(1.0e-6);
    Some([x / n, y / n, z / n, w / n])
}

fn parse_timing_tail(data: &[u8], offset: usize, duration_hint: f32) -> (Vec<f32>, Vec<u32>) {
    if offset >= data.len() {
        return (Vec::new(), Vec::new());
    }

    let mut times = Vec::new();
    let mut cursor = offset;
    let max_reasonable = if duration_hint.is_finite() && duration_hint > 0.0 {
        duration_hint.max(0.5) + 1.0
    } else {
        32.0
    };

    while cursor + 4 <= data.len() {
        let value = f32::from_bits(u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]));

        if !value.is_finite() || value < 0.0 || value > max_reasonable {
            break;
        }

        if let Some(&last) = times.last() {
            if value + 1.0e-4 < last {
                break;
            }
        }

        times.push(value);
        cursor += 4;

        if times.len() >= 256 {
            break;
        }
    }

    if cursor + 4 <= data.len() {
        let zero = u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]);
        if zero == 0 {
            cursor += 4;
        }
    }

    let mut offsets = Vec::new();
    while cursor + 4 <= data.len() {
        let value = u32::from_le_bytes([
            data[cursor],
            data[cursor + 1],
            data[cursor + 2],
            data[cursor + 3],
        ]);

        if value == 0 || value == 0xCCCC_CCCC {
            break;
        }

        if value as usize >= data.len() {
            break;
        }

        offsets.push(value);
        cursor += 4;

        if offsets.len() >= 256 {
            break;
        }
    }

    (times, offsets)
}

fn looks_like_reference_string(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    low.ends_with(".geo")
        || low.ends_with(".dds")
        || low.ends_with(".anm")
        || low.ends_with(".wav")
        || low.ends_with(".ogg")
        || low.contains('/')
        || low.contains('\\')
}

fn extract_ascii_strings(data: &[u8], min_len: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = None;

    for (i, &byte) in data.iter().enumerate() {
        if byte.is_ascii_graphic() || byte == b' ' {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(begin) = start.take() {
            if i - begin >= min_len {
                if let Ok(s) = std::str::from_utf8(&data[begin..i]) {
                    out.push(s.to_owned());
                }
            }
        }
    }

    if let Some(begin) = start {
        if data.len() - begin >= min_len {
            if let Ok(s) = std::str::from_utf8(&data[begin..]) {
                out.push(s.to_owned());
            }
        }
    }

    out
}

fn read_u32(data: &[u8], off: usize) -> Result<u32> {
    let bytes = data
        .get(off..off + 4)
        .with_context(|| format!("Offset 0x{off:08X} is outside the ANM file"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_i16x4(data: &[u8], off: usize) -> Result<[i16; 4]> {
    let bytes = data
        .get(off..off + 8)
        .with_context(|| format!("Offset 0x{off:08X} is outside the ANM file"))?;
    Ok([
        i16::from_le_bytes([bytes[0], bytes[1]]),
        i16::from_le_bytes([bytes[2], bytes[3]]),
        i16::from_le_bytes([bytes[4], bytes[5]]),
        i16::from_le_bytes([bytes[6], bytes[7]]),
    ])
}