//! A display controller's (CRTC's) gamma table, the one DRM-specific part
//! of the night light.
//!
//! The table is written with the legacy gamma ioctl on every driver. On an
//! atomic driver the kernel turns that into a blocking commit of the CRTC's
//! `GAMMA_LUT` property, which waits for a queued page flip rather than
//! failing, and needs no property lookup or blob of our own to keep alive.
//! Smithay never resets `GAMMA_LUT` in its own commits, so the table stays
//! through modesets and the monitor going off and on.
//!
//! The table applies at scanout, to everything on screen including the
//! cursor plane, but not to screenshots or screen sharing, which read the
//! buffers before it.

use smithay::{
    backend::drm::DrmDevice,
    reexports::drm::control::{Device as ControlDevice, crtc},
};
use tracing::{debug, info, warn};

use crate::night_light::temperature::{GammaRamp, NEUTRAL};

/// One CRTC's gamma table. It remembers what it last wrote, so writing the
/// same temperature again costs nothing.
pub struct CrtcGamma {
    crtc: crtc::Handle,
    /// Entries per channel, which the driver decides.
    size: usize,
    /// The temperature in the table, or `None` when unknown: not written
    /// yet, a write failed, or another program may have changed it.
    applied: Option<u32>,
    /// A failed write was already reported; later ones are only debug logs,
    /// since the same failure would otherwise repeat every tick.
    warned: bool,
}

impl CrtcGamma {
    /// The gamma table of `crtc`, or `None` when it has none.
    pub fn new(device: &DrmDevice, crtc: crtc::Handle) -> Option<Self> {
        let size = match device.get_crtc(crtc) {
            Ok(info) => info.gamma_length() as usize,
            Err(err) => {
                info!(%err, "cannot read the display controller; no night light");
                return None;
            }
        };
        if size < 2 {
            info!("the display controller has no gamma table; no night light");
            return None;
        }
        Some(Self {
            crtc,
            size,
            applied: None,
            warned: false,
        })
    }

    /// Tints the screen for `kelvin`. A failed write is retried on the next
    /// call.
    pub fn set(&mut self, device: &DrmDevice, kelvin: u32) {
        if self.applied == Some(kelvin) {
            return;
        }
        let ramp = GammaRamp::new(self.size, kelvin);
        match device.set_gamma(self.crtc, &ramp.red, &ramp.green, &ramp.blue) {
            Ok(()) => {
                debug!(kelvin, "gamma table set");
                self.applied = Some(kelvin);
            }
            Err(err) => {
                if self.warned {
                    debug!(%err, kelvin, "failed to set the gamma table");
                } else {
                    warn!(%err, kelvin, "failed to set the gamma table for the night light");
                    self.warned = true;
                }
                self.applied = None;
            }
        }
    }

    /// Writes the identity table, leaving the screen untinted for whoever
    /// takes the display next. Not the table found at startup: that may be
    /// the tint of an earlier rustwm that crashed.
    pub fn reset(&mut self, device: &DrmDevice) {
        let ramp = GammaRamp::new(self.size, NEUTRAL);
        if let Err(err) = device.set_gamma(self.crtc, &ramp.red, &ramp.green, &ramp.blue) {
            // Usually because the session was already handed over.
            debug!(%err, "failed to reset the gamma table");
        }
        self.applied = None;
    }

    /// Forgets what is in the table, so the next [`set`](Self::set) writes
    /// it. Another program may have changed it while it had the display.
    pub fn forget(&mut self) {
        self.applied = None;
    }
}
