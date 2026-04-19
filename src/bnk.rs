use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const BNK_ENTRY_SIZE: usize = 20;
pub const BNK_FORMAT_PCM16_MONO: u32 = 0x0110_1001;

const PCM_CHANNELS: u16 = 1;
const PCM_BITS_PER_SAMPLE: u16 = 16;
const PCM_BLOCK_ALIGN: u16 = PCM_CHANNELS * (PCM_BITS_PER_SAMPLE / 8);

#[derive(Clone, Debug)]
pub struct BnkEntry {
    pub index: usize,
    pub data_offset: u32,
    pub format_word: u32,
    pub sample_rate: u32,
    pub byte_len: u32,
    pub reserved: u32,
}

impl BnkEntry {
    pub fn data_end(&self) -> u32 {
        self.data_offset.saturating_add(self.byte_len)
    }

    pub fn estimated_duration_seconds(&self) -> Option<f32> {
        if self.sample_rate == 0 || self.format_word != BNK_FORMAT_PCM16_MONO {
            return None;
        }

        let bytes_per_second = self.sample_rate as f32 * PCM_BLOCK_ALIGN as f32;
        Some(self.byte_len as f32 / bytes_per_second)
    }
}

#[derive(Clone, Debug)]
pub struct BnkFile {
    pub path: PathBuf,
    pub file_size: usize,
    pub entry_count: u32,
    pub entries: Vec<BnkEntry>,
    data: Vec<u8>,
}

impl BnkFile {
    pub fn entry_pcm_bytes(&self, index: usize) -> Option<&[u8]> {
        let entry = self.entries.get(index)?;
        let start = entry.data_offset as usize;
        let end = entry.data_end() as usize;
        self.data.get(start..end)
    }

    pub fn entry_wav_bytes(&self, index: usize) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .get(index)
            .context("BNK entry index out of range")?;

        if entry.format_word != BNK_FORMAT_PCM16_MONO {
            bail!(
                "Unsupported BNK entry format 0x{:08X}",
                entry.format_word
            );
        }

        let pcm = self
            .entry_pcm_bytes(index)
            .context("BNK entry payload is out of range")?;

        Ok(build_wav_bytes_pcm16_mono(entry.sample_rate, pcm))
    }
}

pub fn load_bnk(path: &Path) -> Result<BnkFile> {
    let data = fs::read(path)
        .with_context(|| format!("Failed to read BNK file {}", path.display()))?;

    if data.len() < 4 {
        bail!("BNK file is too small");
    }

    let entry_count = read_u32(&data, 0)?;
    let entries_bytes = (entry_count as usize)
        .checked_mul(BNK_ENTRY_SIZE)
        .context("BNK table size overflow")?;

    let table_end = 4usize
        .checked_add(entries_bytes)
        .context("BNK table size overflow")?;

    if data.len() < table_end {
        bail!(
            "BNK is truncated: header says {} entries, but the table would need {} bytes",
            entry_count,
            table_end
        );
    }

    let mut entries = Vec::with_capacity(entry_count as usize);
    let mut prev_end = table_end as u32;

    for index in 0..entry_count as usize {
        let base = 4 + index * BNK_ENTRY_SIZE;

        let entry = BnkEntry {
            index,
            data_offset: read_u32(&data, base)?,
            format_word: read_u32(&data, base + 4)?,
            sample_rate: read_u32(&data, base + 8)?,
            byte_len: read_u32(&data, base + 12)?,
            reserved: read_u32(&data, base + 16)?,
        };

        if index == 0 && entry.data_offset < table_end as u32 {
            bail!("First BNK payload starts before end of table");
        }

        if index > 0 && entry.data_offset != prev_end {
            bail!(
                "Entry {} is not contiguous (expected {}, got {})",
                index,
                prev_end,
                entry.data_offset
            );
        }

        let end = entry.data_end() as usize;
        if end > data.len() {
            bail!(
                "Entry {} points outside the file: offset={} size={} file_size={}",
                index,
                entry.data_offset,
                entry.byte_len,
                data.len()
            );
        }

        prev_end = entry.data_end();
        entries.push(entry);
    }

    Ok(BnkFile {
        path: path.to_path_buf(),
        file_size: data.len(),
        entry_count,
        entries,
        data,
    })
}

pub fn format_name(format_word: u32) -> &'static str {
    match format_word {
        BNK_FORMAT_PCM16_MONO => "PCM16 mono",
        _ => "Unknown",
    }
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    let bytes = data
        .get(offset..offset + 4)
        .context("Unexpected end of BNK file")?;

    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn build_wav_bytes_pcm16_mono(sample_rate: u32, pcm_data: &[u8]) -> Vec<u8> {
    let byte_rate = sample_rate * u32::from(PCM_BLOCK_ALIGN);
    let riff_size = 36u32 + pcm_data.len() as u32;

    let mut out = Vec::with_capacity(44 + pcm_data.len());

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&PCM_CHANNELS.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&PCM_BLOCK_ALIGN.to_le_bytes());
    out.extend_from_slice(&PCM_BITS_PER_SAMPLE.to_le_bytes());

    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm_data.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm_data);

    out
}