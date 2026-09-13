//! Standard OS-level virtual camera output, via the OBS Virtual Camera's own
//! shared-memory pipe (the `virtualcam` crate's "obs" backend) — so the view
//! shows up as a normal webcam in any app (Zoom, Discord, browsers), not just
//! OBS through Spout. Requires OBS to be *installed* (its driver registers
//! the "OBS Virtual Camera" device other apps see) but not running. It's the
//! same device OBS's own "Start Virtual Camera" button feeds, so only one of
//! the two can be sending into it at a time.
//!
//! NOTE: the `virtualcam` crate is AGPL-3.0-licensed — see
//! THIRD-PARTY-NOTICES.md for what that means for this build.

#[cfg(windows)]
mod imp {
    use anyhow::{Result, anyhow};
    use virtualcam::{BackendKind, Camera, PixelFormat};

    pub struct VirtualCamOutput {
        camera: Option<Camera>,
        size: (u32, u32),
    }

    impl VirtualCamOutput {
        pub fn new() -> Self {
            Self { camera: None, size: (0, 0) }
        }

        /// Send a top-down RGBA frame. Lazily (re)opens the camera if the
        /// size changed since the last call.
        pub fn send(&mut self, rgba: &[u8], w: u32, h: u32) -> Result<()> {
            if self.camera.is_none() || self.size != (w, h) {
                let cam = Camera::builder(w, h, 60.0)
                    .format(PixelFormat::RGBA)
                    .backend(BackendKind::Obs)
                    .build()
                    .map_err(|e| anyhow!("{e}"))?;
                self.camera = Some(cam);
                self.size = (w, h);
            }
            self.camera.as_mut().unwrap().send(rgba).map_err(|e| anyhow!("{e}"))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    /// The OBS Virtual Camera transport is Windows-only; elsewhere this is a
    /// no-op so callers don't need to cfg-gate the UI toggle.
    pub struct VirtualCamOutput;

    impl VirtualCamOutput {
        pub fn new() -> Self {
            Self
        }

        pub fn send(&mut self, _rgba: &[u8], _w: u32, _h: u32) -> Result<()> {
            Ok(())
        }
    }
}

pub use imp::VirtualCamOutput;
