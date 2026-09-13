//! Live Spout2 output: sends the same processed (cropped/lens-flattened) BGRA
//! frame the on-screen view and recordings use, as a local GPU/CPU texture
//! share. Add "Quest scrcpy" as a source in OBS via the Spout2 Plugin
//! (obsproject.com plugin "Spout2") — no Window Capture, no encoding.

#[cfg(windows)]
mod imp {
    use anyhow::{Result, anyhow};
    use spout2::dx::Sender;

    pub struct SpoutOutput {
        name: String,
        sender: Option<Sender>,
    }

    impl SpoutOutput {
        pub fn new(name: impl Into<String>) -> Self {
            Self { name: name.into(), sender: None }
        }

        /// Send a top-down BGRA frame. Lazily (re)creates the sender.
        pub fn send(&mut self, bgra: &[u8], w: u32, h: u32) -> Result<()> {
            if self.sender.is_none() {
                self.sender =
                    Some(Sender::new(&self.name).map_err(|e| anyhow!("{e}"))?);
            }
            self.sender
                .as_mut()
                .unwrap()
                .send_image(bgra, w, h)
                .map_err(|e| anyhow!("{e}"))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    /// Spout is Windows-only; elsewhere this is a no-op so callers don't need
    /// to cfg-gate the UI toggle.
    pub struct SpoutOutput;

    impl SpoutOutput {
        pub fn new(_name: impl Into<String>) -> Self {
            Self
        }

        pub fn send(&mut self, _bgra: &[u8], _w: u32, _h: u32) -> Result<()> {
            Ok(())
        }
    }
}

pub use imp::SpoutOutput;
