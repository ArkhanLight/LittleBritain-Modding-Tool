use anyhow::{Context, Result};
use eframe::egui;
use std::{fs::File, path::Path};

#[derive(Clone)]
pub struct DdsPreview {
    pub texture: egui::TextureHandle,
    pub width: usize,
    pub height: usize,
    pub mipmaps: u32,
    pub rgba_pixels: Vec<u8>,
}

pub fn load_dds_preview(ctx: &egui::Context, path: &Path) -> Result<DdsPreview> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open DDS file {}", path.display()))?;

    let dds = image_dds::ddsfile::Dds::read(file)
        .with_context(|| format!("Failed to parse DDS file {}", path.display()))?;

    let rgba = image_dds::image_from_dds(&dds, 0)
        .with_context(|| format!("Failed to decode DDS pixels from {}", path.display()))?;

    let width = rgba.width() as usize;
    let height = rgba.height() as usize;
    let rgba_pixels = rgba.into_raw();

    let color_image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba_pixels);

    let texture = ctx.load_texture(
        path.display().to_string(),
        color_image,
        egui::TextureOptions::NEAREST,
    );

    Ok(DdsPreview {
        texture,
        width,
        height,
        mipmaps: dds.get_num_mipmap_levels(),
        rgba_pixels,
    })
}
