use anyhow::{Context, Result};
use eframe::egui;
use std::{fs::File, path::Path};

#[derive(Clone, Debug)]
pub struct DdsRawPreview {
    pub width: usize,
    pub height: usize,
    pub mipmaps: u32,
    pub rgba_pixels: Vec<u8>,
}

#[derive(Clone)]
pub struct DdsPreview {
    pub texture: egui::TextureHandle,
    pub width: usize,
    pub height: usize,
    pub mipmaps: u32,
    pub rgba_pixels: Vec<u8>,
}

pub fn load_dds_raw_preview(path: &Path) -> Result<DdsRawPreview> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open DDS file {}", path.display()))?;

    let dds = image_dds::ddsfile::Dds::read(file)
        .with_context(|| format!("Failed to parse DDS file {}", path.display()))?;

    let rgba = image_dds::image_from_dds(&dds, 0)
        .with_context(|| format!("Failed to decode DDS pixels from {}", path.display()))?;

    Ok(DdsRawPreview {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        mipmaps: dds.get_num_mipmap_levels(),
        rgba_pixels: rgba.into_raw(),
    })
}

pub fn dds_preview_from_raw(
    ctx: &egui::Context,
    texture_name: impl Into<String>,
    raw: DdsRawPreview,
) -> DdsPreview {
    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [raw.width, raw.height],
        &raw.rgba_pixels,
    );

    let texture = ctx.load_texture(
        texture_name.into(),
        color_image,
        egui::TextureOptions::NEAREST,
    );

    DdsPreview {
        texture,
        width: raw.width,
        height: raw.height,
        mipmaps: raw.mipmaps,
        rgba_pixels: raw.rgba_pixels,
    }
}

pub fn load_dds_preview(ctx: &egui::Context, path: &Path) -> Result<DdsPreview> {
    let raw = load_dds_raw_preview(path)?;
    Ok(dds_preview_from_raw(ctx, path.display().to_string(), raw))
}
