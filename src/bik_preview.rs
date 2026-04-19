use anyhow::{anyhow, bail, Context, Result};
use eframe::egui::ColorImage;
use ffmpeg_next as ffmpeg;
use ffmpeg::{
    codec, format, frame, media,
    software::scaling::{context::Context as Scaler, flag::Flags},
    util::format::pixel::Pixel,
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

pub fn load_bik_preview(path: &Path) -> Result<BikPreview> {
    ensure_ffmpeg_init()?;

    let mut input = format::input(path)
        .with_context(|| format!("Failed to open BIK file {}", path.display()))?;

    let stream = input
        .streams()
        .best(media::Type::Video)
        .ok_or_else(|| anyhow!("No video stream found in {}", path.display()))?;

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