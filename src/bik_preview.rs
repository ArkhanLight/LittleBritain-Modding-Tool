use anyhow::{anyhow, bail, Context, Result};
use eframe::egui::ColorImage;
use ffmpeg_next as ffmpeg;
use ffmpeg::{
    codec, format, frame, media,
    software::{
        resampling::context::Context as Resampler,
        scaling::{context::Context as Scaler, flag::Flags},
    },
    util::format::{
        pixel::Pixel,
        sample::{Sample, Type as SampleType},
    },
    ChannelLayout,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver},
        OnceLock,
    },
    thread,
};

static FFMPEG_INIT: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_ffmpeg_init() -> Result<()> {
    let init = FFMPEG_INIT.get_or_init(|| {
        ffmpeg::init()
            .map_err(|e| format!("FFmpeg init failed: {e:?}"))
            .map(|_| ())
    });

    match init {
        Ok(()) => Ok(()),
        Err(err) => Err(anyhow!(err.clone())),
    }
}

#[derive(Clone)]
pub struct BikPreview {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub duration_seconds: Option<f32>,
    pub has_audio: bool,
    pub first_frame: ColorImage,
}

impl BikPreview {
    pub fn total_duration_seconds(&self) -> f32 {
        self.duration_seconds.unwrap_or(0.0).max(0.0)
    }

    pub fn estimated_frame_count(&self) -> usize {
        if self.fps > 0.0 {
            (self.total_duration_seconds() * self.fps).round().max(1.0) as usize
        } else {
            1
        }
    }
}

pub enum BikWorkerEvent {
    Frame {
        frame_index: usize,
        time_seconds: f32,
        image: ColorImage,
    },
    Finished,
    Error(String),
}

fn rational_to_f32(r: ffmpeg::Rational) -> Option<f32> {
    let num = r.numerator();
    let den = r.denominator();
    if den == 0 {
        None
    } else {
        Some(num as f32 / den as f32)
    }
}

fn frame_to_color_image(frame: &frame::Video) -> Result<ColorImage> {
    let width = frame.width() as usize;
    let height = frame.height() as usize;
    let stride = frame.stride(0);
    let data = frame.data(0);

    if data.is_empty() || width == 0 || height == 0 {
        bail!("Decoded frame is empty");
    }

    let mut rgba = Vec::with_capacity(width * height * 4);

    for y in 0..height {
        let row_start = y * stride;
        let row_end = row_start + width * 4;

        if row_end > data.len() {
            bail!("Decoded frame buffer is smaller than expected");
        }

        rgba.extend_from_slice(&data[row_start..row_end]);
    }

    Ok(ColorImage::from_rgba_unmultiplied([width, height], &rgba))
}

fn append_resampled_pcm16_frame(frame: &frame::Audio, pcm_data: &mut Vec<u8>) -> Result<()> {
    if frame.format() != Sample::I16(SampleType::Packed) {
        bail!("Resampled audio frame is not packed 16-bit PCM");
    }

    match frame.channels() {
        1 => {
            for sample in frame.plane::<i16>(0) {
                pcm_data.extend_from_slice(&sample.to_le_bytes());
            }
        }
        2 => {
            for (left, right) in frame.plane::<(i16, i16)>(0) {
                pcm_data.extend_from_slice(&left.to_le_bytes());
                pcm_data.extend_from_slice(&right.to_le_bytes());
            }
        }
        channels => {
            bail!("Unsupported resampled channel count: {channels}");
        }
    }

    Ok(())
}

fn build_wav_bytes_pcm16(sample_rate: u32, channels: u16, pcm_data: &[u8]) -> Vec<u8> {
    let bits_per_sample = 16u16;
    let block_align = channels * (bits_per_sample / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let riff_size = 36u32 + pcm_data.len() as u32;

    let mut out = Vec::with_capacity(44 + pcm_data.len());

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits_per_sample.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm_data.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm_data);

    out
}

pub fn extract_bik_audio_wav(path: &Path) -> Result<Option<Vec<u8>>> {
    ensure_ffmpeg_init()?;

    let mut input = format::input(path)
        .with_context(|| format!("Failed to open BIK file {}", path.display()))?;

    let stream = match input.streams().best(media::Type::Audio) {
        Some(stream) => stream,
        None => return Ok(None),
    };

    let stream_index = stream.index();

    let context = codec::context::Context::from_parameters(stream.parameters())
        .context("Failed to create audio codec context from stream parameters")?;

    let mut decoder = context
        .decoder()
        .audio()
        .context("Failed to open audio decoder")?;

    let src_rate = decoder.rate().max(1);
    let src_layout = if decoder.channel_layout().is_empty() {
        ChannelLayout::default(decoder.channels().into())
    } else {
        decoder.channel_layout()
    };

    let dst_rate = src_rate;
    let dst_format = Sample::I16(SampleType::Packed);
    let dst_layout = ChannelLayout::STEREO;

    let mut resampler = Resampler::get(
        decoder.format(),
        src_layout,
        src_rate,
        dst_format,
        dst_layout,
        dst_rate,
    )
    .context("Failed to create FFmpeg audio resampler")?;

    let mut decoded = frame::Audio::empty();
    let mut pcm_data = Vec::new();

    let mut process_decoded_frames = |decoder: &mut ffmpeg::decoder::Audio| -> Result<()> {
        while decoder.receive_frame(&mut decoded).is_ok() {
            let mut resampled = frame::Audio::empty();
            resampler
                .run(&decoded, &mut resampled)
                .context("Failed to resample audio frame")?;

            append_resampled_pcm16_frame(&resampled, &mut pcm_data)?;
        }

        Ok(())
    };

    for (packet_stream, packet) in input.packets() {
        if packet_stream.index() != stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Failed to send audio packet to decoder")?;

        process_decoded_frames(&mut decoder)?;
    }

    decoder.send_eof().ok();
    process_decoded_frames(&mut decoder)?;

    loop {
        let mut flushed = frame::Audio::empty();
        match resampler.flush(&mut flushed) {
            Ok(Some(_)) => append_resampled_pcm16_frame(&flushed, &mut pcm_data)?,
            Ok(None) => break,
            Err(_) => break,
        }
    }

    if pcm_data.is_empty() {
        return Ok(None);
    }

    Ok(Some(build_wav_bytes_pcm16(
        dst_rate,
        dst_layout.channels() as u16,
        &pcm_data,
    )))
}

pub fn load_bik_preview(path: &Path) -> Result<BikPreview> {
    ensure_ffmpeg_init()?;

    let mut input = format::input(path)
        .with_context(|| format!("Failed to open BIK file {}", path.display()))?;

    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("No video stream found in {}", path.display()))?;

    let has_audio = input.streams().best(media::Type::Audio).is_some();

    let stream_index = stream.index();
    let fps = rational_to_f32(stream.rate())
        .filter(|fps| *fps > 0.0)
        .unwrap_or(25.0);

    let duration_seconds = {
        let dur = input.duration();
        if dur > 0 {
            Some(dur as f32 / ffmpeg::ffi::AV_TIME_BASE as f32)
        } else {
            None
        }
    };

    let context = codec::context::Context::from_parameters(stream.parameters())
        .context("Failed to create codec context from stream parameters")?;

    let mut decoder = context
        .decoder()
        .video()
        .context("Failed to open video decoder")?;

    let src_width = decoder.width();
    let src_height = decoder.height();

    let mut scaler = Scaler::get(
        decoder.format(),
        src_width,
        src_height,
        Pixel::RGBA,
        src_width,
        src_height,
        Flags::BILINEAR,
    )
    .context("Failed to create FFmpeg scaler")?;

    let mut decoded = frame::Video::empty();
    let mut rgba = frame::Video::empty();
    let mut got_frame = false;

    for (packet_stream, packet) in input.packets() {
        if packet_stream.index() != stream_index {
            continue;
        }

        decoder
            .send_packet(&packet)
            .context("Failed to send packet to decoder")?;

        while decoder.receive_frame(&mut decoded).is_ok() {
            scaler
                .run(&decoded, &mut rgba)
                .context("Failed to convert frame to RGBA")?;
            got_frame = true;
            break;
        }

        if got_frame {
            break;
        }
    }

    if !got_frame {
        decoder.send_eof().ok();

        while decoder.receive_frame(&mut decoded).is_ok() {
            scaler
                .run(&decoded, &mut rgba)
                .context("Failed to convert frame to RGBA")?;
            got_frame = true;
            break;
        }
    }

    if !got_frame {
        bail!("Could not decode a preview frame from {}", path.display());
    }

    Ok(BikPreview {
        path: path.to_path_buf(),
        width: src_width,
        height: src_height,
        fps,
        duration_seconds,
        has_audio,
        first_frame: frame_to_color_image(&rgba)?,
    })
}

pub fn spawn_bik_decoder(path: PathBuf, start_frame: usize) -> Receiver<BikWorkerEvent> {
    let (tx, rx) = mpsc::sync_channel(4);

    thread::spawn(move || {
        let result = (|| -> Result<()> {
            ensure_ffmpeg_init()?;

            let mut input = format::input(&path)
                .with_context(|| format!("Failed to open BIK file {}", path.display()))?;

            let stream = input
                .streams()
                .best(media::Type::Video)
                .ok_or_else(|| anyhow!("No video stream found in {}", path.display()))?;

            let stream_index = stream.index();
            let fps = rational_to_f32(stream.rate())
                .filter(|fps| *fps > 0.0)
                .unwrap_or(25.0);

            let context = codec::context::Context::from_parameters(stream.parameters())
                .context("Failed to create codec context from stream parameters")?;

            let mut decoder = context
                .decoder()
                .video()
                .context("Failed to open video decoder")?;

            let src_width = decoder.width();
            let src_height = decoder.height();

            let mut scaler = Scaler::get(
                decoder.format(),
                src_width,
                src_height,
                Pixel::RGBA,
                src_width,
                src_height,
                Flags::BILINEAR,
            )
            .context("Failed to create FFmpeg scaler")?;

            let mut decoded = frame::Video::empty();
            let mut rgba = frame::Video::empty();
            let mut decoded_index = 0usize;

            for (packet_stream, packet) in input.packets() {
                if packet_stream.index() != stream_index {
                    continue;
                }

                decoder
                    .send_packet(&packet)
                    .context("Failed to send packet to decoder")?;

                while decoder.receive_frame(&mut decoded).is_ok() {
                    if decoded_index < start_frame {
                        decoded_index += 1;
                        continue;
                    }

                    scaler
                        .run(&decoded, &mut rgba)
                        .context("Failed to convert frame to RGBA")?;

                    let image = frame_to_color_image(&rgba)?;
                    let time_seconds = if fps > 0.0 {
                        decoded_index as f32 / fps
                    } else {
                        0.0
                    };

                    if tx
                        .send(BikWorkerEvent::Frame {
                            frame_index: decoded_index,
                            time_seconds,
                            image,
                        })
                        .is_err()
                    {
                        return Ok(());
                    }

                    decoded_index += 1;
                }
            }

            decoder.send_eof().ok();

            while decoder.receive_frame(&mut decoded).is_ok() {
                if decoded_index < start_frame {
                    decoded_index += 1;
                    continue;
                }

                scaler
                    .run(&decoded, &mut rgba)
                    .context("Failed to convert frame to RGBA")?;

                let image = frame_to_color_image(&rgba)?;
                let time_seconds = if fps > 0.0 {
                    decoded_index as f32 / fps
                } else {
                    0.0
                };

                if tx
                    .send(BikWorkerEvent::Frame {
                        frame_index: decoded_index,
                        time_seconds,
                        image,
                    })
                    .is_err()
                {
                    return Ok(());
                }

                decoded_index += 1;
            }

            let _ = tx.send(BikWorkerEvent::Finished);
            Ok(())
        })();

        if let Err(err) = result {
            let _ = tx.send(BikWorkerEvent::Error(err.to_string()));
        }
    });

    rx
}