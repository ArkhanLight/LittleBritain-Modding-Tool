use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Folder,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetCategory {
    Texture,
    Model,
    Animation,
    Particle,
    AudioStream,
    AudioBank,
    Lighting,
    Scene,
    Log,
    Unknown,
}

pub fn classify_extension(ext: &str) -> AssetCategory {
    match ext.to_ascii_lowercase().as_str() {
        "dds" => AssetCategory::Texture,
        "geo" => AssetCategory::Model,
        "anm" => AssetCategory::Animation,
        "ps2" | "psf" => AssetCategory::Particle, // treat .psf as particle-related for now
        "ogg" | "wav" => AssetCategory::AudioStream,
        "bnk" => AssetCategory::AudioBank,
        "lgt" => AssetCategory::Lighting,
        "scn" => AssetCategory::Scene,
        "log" => AssetCategory::Log,
        _ => AssetCategory::Unknown,
    }
}

pub fn classify_path(path: &Path) -> AssetCategory {
    path.extension()
        .and_then(|e| e.to_str())
        .map(classify_extension)
        .unwrap_or(AssetCategory::Unknown)
}

pub fn category_name(category: AssetCategory) -> &'static str {
    match category {
        AssetCategory::Texture => "Texture",
        AssetCategory::Model => "Model",
        AssetCategory::Animation => "Animation",
        AssetCategory::Particle => "Particle",
        AssetCategory::AudioStream => "Audio Stream",
        AssetCategory::AudioBank => "Audio Bank",
        AssetCategory::Lighting => "Lighting",
        AssetCategory::Scene => "Scene",
        AssetCategory::Log => "Log",
        AssetCategory::Unknown => "Unknown",
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct FileNode {
    pub name: String,
    pub path: PathBuf,
    pub kind: NodeKind,
    pub size: Option<u64>,
    pub category: Option<AssetCategory>,
    pub children: Vec<FileNode>,
}

pub fn scan_game_data(game_root: &Path) -> Result<Vec<FileNode>> {
    let data_path = game_root.join("Data");

    if !data_path.is_dir() {
        anyhow::bail!("Could not find a Data folder at {}", data_path.display());
    }

    read_nodes(&data_path)
}

fn read_nodes(dir: &Path) -> Result<Vec<FileNode>> {
    let mut out = Vec::new();

    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .with_context(|| format!("Failed to enumerate {}", dir.display()))?;

    entries.sort_by_key(|e| {
        let is_file = e.file_type().map(|t| t.is_file()).unwrap_or(false);
        (is_file, e.file_name().to_string_lossy().to_lowercase())
    });

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let metadata = entry.metadata()?;
        let name = entry.file_name().to_string_lossy().to_string();

        if file_type.is_dir() {
            out.push(FileNode {
                name,
                path: path.clone(),
                kind: NodeKind::Folder,
                size: None,
                category: None,
                children: read_nodes(&path)?,
            });
        } else {
            out.push(FileNode {
                name,
                path: path.clone(),
                kind: NodeKind::File,
                size: Some(metadata.len()),
                category: Some(classify_path(&path)),
                children: Vec::new(),
            });
        }
    }

    Ok(out)
}