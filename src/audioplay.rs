//! AAC playback with two interchangeable backends behind one API:
//!
//! - [`ffmpeg`] — wraps access units in ADTS and pipes them through the
//!   `ffmpeg` CLI (cross-platform; chosen when ffmpeg is on PATH).
//! - [`mf`] — the built-in Media Foundation AAC decoder (Windows fallback).
//!
//! Both decode AAC-LC and play through cpal; callers just use [`AacPlayer`].

mod ffmpeg;
#[cfg(windows)]
mod mf;

use anyhow::Result;
use cpal::traits::HostTrait;

pub enum AacPlayer {
    Ffmpeg(ffmpeg::FfmpegPlayer),
    #[cfg(windows)]
    Mf(mf::MfPlayer),
}

impl AacPlayer {
    /// `device` names a cpal output device to play through; `None`/no match
    /// falls back to the system default.
    pub fn new(device: Option<&str>) -> Result<Self> {
        if crate::ffmpeg::use_for_playback() {
            return Ok(Self::Ffmpeg(ffmpeg::FfmpegPlayer::new(device)?));
        }
        #[cfg(windows)]
        {
            return Ok(Self::Mf(mf::MfPlayer::new(device)?));
        }
        #[cfg(not(windows))]
        anyhow::bail!("no audio backend — install ffmpeg and make sure it is on your PATH")
    }

    /// Feed an access unit, or the AudioSpecificConfig when `is_config`.
    pub fn feed(&mut self, data: &[u8], is_config: bool) -> Result<()> {
        match self {
            Self::Ffmpeg(p) => p.feed(data, is_config),
            #[cfg(windows)]
            Self::Mf(p) => p.feed(data, is_config),
        }
    }
}

/// Every cpal output device name on the system, for the Settings picker.
pub fn output_device_names() -> Vec<String> {
    let Ok(devices) = cpal::default_host().output_devices() else {
        return Vec::new();
    };
    devices.map(|d| d.to_string()).collect()
}

/// Resolve `name` to a cpal output device, falling back to the system default
/// when it's `None`/empty or no longer exists (e.g. a virtual cable that got
/// uninstalled) — never fails outright just because a saved device is gone.
pub(crate) fn resolve_output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    if let Some(name) = name.filter(|n| !n.is_empty())
        && let Ok(devices) = host.output_devices()
        && let Some(d) = devices.into_iter().find(|d| d.to_string() == name)
    {
        return Ok(d);
    }
    host.default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default audio output device"))
}
