//! wlr-output-power-management: lets an idle daemon turn the monitor off
//! and on, for example `swayidle timeout 600 'wlopm --off "*"'`.
//!
//! Smithay has no helper for this protocol, so its two interfaces are
//! implemented here. Any keyboard or mouse input also turns the monitor
//! back on.

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::output_power_management::v1::server::{
            zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
            zwlr_output_power_v1::{self, Mode, ZwlrOutputPowerV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
        },
    },
};
use tracing::info;

use crate::state::{Backend, State};

/// The power controls clients have created, and which outputs are off.
#[derive(Debug, Default)]
pub struct OutputPowerState {
    controls: Vec<(ZwlrOutputPowerV1, Output)>,
    off: Vec<Output>,
}

impl OutputPowerState {
    pub fn new(dh: &DisplayHandle) -> Self {
        dh.create_global::<State, ZwlrOutputPowerManagerV1, _>(1, ());
        Self::default()
    }
}

impl State {
    pub fn is_powered_off(&self, output: &Output) -> bool {
        self.output_power.off.contains(output)
    }

    /// Turns `output` on or off and tells every power control for it.
    pub fn set_output_power(&mut self, output: &Output, on: bool) {
        if on == !self.is_powered_off(output) {
            return;
        }
        info!(output = output.name(), on, "output power changed");
        if on {
            self.output_power.off.retain(|o| o != output);
        } else {
            self.output_power.off.push(output.clone());
        }
        let mode = if on { Mode::On } else { Mode::Off };
        for (control, _) in self
            .output_power
            .controls
            .iter()
            .filter(|(_, o)| o == output)
        {
            control.mode(mode);
        }
        if let Backend::Udev(_) = self.backend {
            crate::udev::set_power(self, output, on);
        }
        // A lock waiting for its first frame is complete: nothing is shown.
        if !self.is_showing() {
            self.lock_frame_shown();
        }
    }

    /// Turns every output that is off back on, on user input.
    pub fn power_on_outputs(&mut self) {
        for output in self.output_power.off.clone() {
            self.set_output_power(&output, true);
        }
    }

    /// Invalidates the power controls of an output that went away.
    pub fn output_power_removed(&mut self, output: &Output) {
        self.output_power.off.retain(|o| o != output);
        self.output_power.controls.retain(|(control, o)| {
            if o == output {
                control.failed();
            }
            o != output
        });
    }
}

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } = request {
            let control = data_init.init(id, ());
            match Output::from_resource(&output) {
                Some(output) => {
                    let on = !state.is_powered_off(&output);
                    control.mode(if on { Mode::On } else { Mode::Off });
                    state.output_power.controls.push((control, output));
                }
                None => control.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for State {
    fn request(
        state: &mut Self,
        _client: &Client,
        control: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let zwlr_output_power_v1::Request::SetMode { mode } = request else {
            return;
        };
        let Some(output) = state
            .output_power
            .controls
            .iter()
            .find(|(c, _)| c == control)
            .map(|(_, output)| output.clone())
        else {
            // The output went away and the control was told it failed.
            return;
        };
        match mode {
            WEnum::Value(Mode::On) => state.set_output_power(&output, true),
            WEnum::Value(Mode::Off) => state.set_output_power(&output, false),
            _ => control.post_error(zwlr_output_power_v1::Error::InvalidMode, "unknown mode"),
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        control: &ZwlrOutputPowerV1,
        _data: &(),
    ) {
        state.output_power.controls.retain(|(c, _)| c != control);
    }
}
