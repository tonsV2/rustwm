//! Smithay protocol handlers. Each `delegate_*!` macro wires a Wayland
//! protocol to the matching handler trait on `State`.

mod capture;
mod compositor;
mod foreign_toplevel;
mod idle;
mod layer_shell;
mod output_power;
mod session_lock;
mod xdg_activation;
mod xdg_shell;

use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::ImportDma},
    delegate_alpha_modifier, delegate_content_type, delegate_cursor_shape, delegate_data_device,
    delegate_dmabuf, delegate_fractional_scale, delegate_output, delegate_primary_selection,
    delegate_seat, delegate_single_pixel_buffer, delegate_viewporter, delegate_xdg_decoration,
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        wayland_server::{Resource, protocol::wl_surface::WlSurface},
    },
    wayland::{
        compositor::with_states,
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
        },
        shell::xdg::{ToplevelSurface, decoration::XdgDecorationHandler},
        tablet_manager::TabletSeatHandler,
    },
};
use tracing::info;

pub use idle::IdleNotifyState;
pub use layer_shell::handle_commit as handle_layer_commit;
pub use output_power::OutputPowerState;
pub use session_lock::{LOCKED_BACKGROUND, ScreenLock};
pub use xdg_shell::handle_commit as handle_xdg_commit;

use crate::state::State;

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        // Clipboard offers go to whichever client has keyboard focus.
        let dh = &self.display_handle;
        let client = focused.and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, client.clone());
        set_primary_focus(dh, seat, client);
    }
}
delegate_seat!(State);

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}
delegate_data_device!(State);

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(State);

impl OutputHandler for State {}
delegate_output!(State);

delegate_viewporter!(State);
delegate_single_pixel_buffer!(State);
delegate_alpha_modifier!(State);
delegate_content_type!(State);

impl TabletSeatHandler for State {}
delegate_cursor_shape!(State);

/// Tells each surface the output scale, so clients that support fractional
/// scaling draw at exactly that resolution instead of a blurry whole
/// number. With a single output every surface gets that output's scale.
impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let Some(scale) = self
            .primary_output()
            .map(|o| o.current_scale().fractional_scale())
        else {
            return;
        };
        with_states(&surface, |states| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        });
    }
}
delegate_fractional_scale!(State);

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        if self.backend.renderer().import_dmabuf(&dmabuf, None).is_ok() {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}
delegate_dmabuf!(State);

/// Always asks for server-side decorations. The compositor draws none, so
/// clients that honour this (Qt, foot, ...) show no title bar at all.
impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        set_server_side(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecorationMode) {
        set_server_side(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_server_side(&toplevel);
    }
}
delegate_xdg_decoration!(State);

fn set_server_side(toplevel: &ToplevelSurface) {
    toplevel.with_pending_state(|state| {
        state.decoration_mode = Some(DecorationMode::ServerSide);
    });
    if toplevel.is_initial_configure_sent() {
        toplevel.send_pending_configure();
    }
}

/// Logs a window event with the client's app id and title.
pub fn log_window(event: &str, id: u64, toplevel: &ToplevelSurface) {
    use smithay::wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData};

    let (app_id, title) = with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|data| data.lock().ok())
            .map(|data| (data.app_id.clone(), data.title.clone()))
            .unwrap_or_default()
    });
    info!(window = id, app_id, title, "{event}");
}
