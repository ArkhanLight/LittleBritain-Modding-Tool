use anyhow::{Context, Result};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::{fs::File, io::Cursor, path::Path, time::Duration};

pub struct AudioPlayer {
    _device_sink: MixerDeviceSink,
    player: Player,
    current_path: Option<String>,
    duration: Option<Duration>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let device_sink = DeviceSinkBuilder::open_default_sink()
            .context("Failed to open default audio output device")?;

        let player = Player::connect_new(device_sink.mixer());

        Ok(Self {
            _device_sink: device_sink,
            player,
            current_path: None,
            duration: None,
        })
    }

    pub fn play_file(&mut self, path: &Path) -> Result<()> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open audio file {}", path.display()))?;

        let decoder = Decoder::try_from(file)
            .with_context(|| format!("Failed to decode audio file {}", path.display()))?;

        let duration = decoder.total_duration();

        self.player.stop();
        self.player.append(decoder);
        self.player.play();

        self.current_path = Some(path.display().to_string());
        self.duration = duration;

        Ok(())
    }

    pub fn play_data(&mut self, label: impl Into<String>, data: Vec<u8>) -> Result<()> {
        let decoder = Decoder::try_from(Cursor::new(data))
            .context("Failed to decode in-memory audio")?;

        let duration = decoder.total_duration();

        self.player.stop();
        self.player.append(decoder);
        self.player.play();

        self.current_path = Some(label.into());
        self.duration = duration;

        Ok(())
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.play();
    }

    pub fn stop(&self) {
        self.player.stop();
    }

    pub fn seek(&self, pos: Duration) {
        let _ = self.player.try_seek(pos);
    }

    pub fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    pub fn is_empty(&self) -> bool {
        self.player.empty()
    }

    pub fn volume(&self) -> f32 {
        self.player.volume()
    }

    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }

    pub fn duration(&self) -> Option<Duration> {
        self.duration
    }

    pub fn current_path(&self) -> Option<&str> {
        self.current_path.as_deref()
    }
}