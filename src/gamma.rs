//! A display controller's (CRTC's) gamma table, the one DRM-specific part
//! of the night light.
//!
//! The table is written with the legacy gamma ioctl on every driver. On an
//! atomic driver the kernel turns that into a blocking commit of the CRTC's
//! `GAMMA_LUT` property, which waits for a queued page flip rather than
//! failing, and needs no property lookup or blob of our own to keep alive.
//! That commit also clears the CRTC's `DEGAMMA_LUT` and `CTM`. Smithay never
//! sets any of the three in its own commits, so the table stays through
//! modesets and the monitor going off and on.
//!
//! Waiting for the flip means a write can hold up the event loop for up to
//! a frame. A fade makes about ten writes over its second, so it can cost a
//! few frames; the schedule alone writes at most every 30 seconds, and only
//! when the table changes.
//!
//! The table applies at scanout, to everything on screen including the
//! cursor plane, but not to screenshots or screen sharing, which read the
//! buffers before it.

use std::io;

use smithay::{
    backend::drm::{DrmDevice, DrmDeviceFd},
    reexports::drm::{
        ClientCapability, Device as BasicDevice,
        control::{
            AtomicCommitFlags, Device as ControlDevice, atomic::AtomicModeReq, crtc, property,
        },
    },
};
use tracing::{debug, info, warn};

use crate::night_light::temperature::{GammaRamp, NEUTRAL};

/// The CRTC properties the gamma ioctl replaces or clears.
const COLOR_PROPERTIES: [&[u8]; 3] = [b"GAMMA_LUT", b"DEGAMMA_LUT", b"CTM"];

/// Takes the colour tables off every CRTC. Call it on the device before
/// Smithay's `DrmDevice` opens it.
///
/// Smithay remembers the display state it finds at startup, and on exit
/// commits all of it back at once. Those tables are blobs that the kernel
/// frees once nothing uses them, so a blob left by a program that has
/// exited, such as a crashed rustwm's tint, lives only as long as the CRTC
/// shows it. Our first gamma write would free it, and Smithay's restore
/// would then name a blob that no longer exists, which fails the whole
/// commit, not just the gamma part: the console might not come back.
/// Clearing the tables first makes Smithay remember "no table", which is
/// always valid, and also what we want left behind: an untinted screen.
///
/// Only for atomic drivers. Smithay's legacy restore does not touch gamma,
/// so there [`CrtcGamma::reset`] on exit is what untints the screen.
pub fn clear_color_tables(fd: &DrmDeviceFd) {
    // Smithay then drives the device the legacy way, and turning on the
    // atomic API on its file would defeat that escape hatch.
    if smithay_forces_legacy() {
        return;
    }
    // Smithay asks for the same capability next and falls back to legacy
    // when it is refused.
    if let Err(err) = fd.set_client_capability(ClientCapability::Atomic, true) {
        debug!(%err, "not an atomic driver; colour tables left as they are");
        return;
    }
    match try_clear_color_tables(fd) {
        Ok(0) => {}
        Ok(cleared) => info!(cleared, "cleared colour tables left by another program"),
        Err(err) => warn!(
            %err,
            "failed to clear colour tables; restoring the display on exit may fail"
        ),
    }
}

/// Smithay's own test for `SMITHAY_USE_LEGACY`, in `DrmDevice::new`.
fn smithay_forces_legacy() -> bool {
    std::env::var("SMITHAY_USE_LEGACY").is_ok_and(|value| {
        let value = value.to_lowercase();
        value == "1" || value == "true" || value == "yes" || value == "y"
    })
}

/// Returns how many properties it cleared. The atomic capability must be on.
fn try_clear_color_tables(fd: &DrmDeviceFd) -> io::Result<usize> {
    let mut request = AtomicModeReq::new();
    let mut cleared = 0;
    for &crtc in fd.resource_handles()?.crtcs() {
        let properties = fd.get_properties(crtc)?;
        let (handles, values) = properties.as_props_and_values();
        for (&handle, &value) in handles.iter().zip(values) {
            // 0 is no blob: nothing to clear.
            if value == 0 {
                continue;
            }
            let name = fd.get_property(handle)?.name().to_bytes().to_vec();
            if COLOR_PROPERTIES.contains(&name.as_slice()) {
                request.add_property(crtc, handle, property::Value::Blob(0));
                cleared += 1;
            }
        }
    }
    if cleared > 0 {
        // Changing colour tables needs no modeset.
        fd.atomic_commit(AtomicCommitFlags::empty(), request)?;
    }
    Ok(cleared)
}

/// One CRTC's gamma table. It remembers what it last wrote, so writing the
/// same table again costs nothing.
pub struct CrtcGamma {
    crtc: crtc::Handle,
    /// Entries per channel, which the driver decides.
    size: usize,
    /// The table last written, or `None` when unknown: not written yet, a
    /// write failed, or another program may have changed it. Comparing
    /// tables rather than temperatures also skips fade steps too small to
    /// change any entry.
    written: Option<GammaRamp>,
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
            written: None,
            warned: false,
        })
    }

    /// Tints the screen for `kelvin`. A failed write is retried on the next
    /// call.
    pub fn set(&mut self, device: &DrmDevice, kelvin: u32) {
        let ramp = GammaRamp::new(self.size, kelvin);
        if self.written.as_ref() == Some(&ramp) {
            return;
        }
        match device.set_gamma(self.crtc, &ramp.red, &ramp.green, &ramp.blue) {
            Ok(()) => {
                debug!(kelvin, "gamma table set");
                self.written = Some(ramp);
            }
            Err(err) => {
                if self.warned {
                    debug!(%err, kelvin, "failed to set the gamma table");
                } else {
                    warn!(%err, kelvin, "failed to set the gamma table for the night light");
                    self.warned = true;
                }
                self.written = None;
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
        // Another program may change it before we write again.
        self.written = None;
    }

    /// Forgets what is in the table, so the next [`set`](Self::set) writes
    /// it. Another program may have changed it while it had the display.
    pub fn forget(&mut self) {
        self.written = None;
    }
}
