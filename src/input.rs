//! Routes input events to keybindings and clients.

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    desktop::layer_map_for_output,
    input::{
        keyboard::{FilterResult, Keysym},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::shell::wlr_layer::Layer,
};

use crate::{
    focus::WindowId,
    keyboard::{Action, Key, Mods},
    layout::{Direction, Layout},
    state::{Backend, State},
};
use tracing::info;

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => self.on_keyboard::<I>(event),
            InputEvent::PointerMotion { event } => self.on_pointer_motion::<I>(event),
            InputEvent::PointerMotionAbsolute { event } => {
                self.on_pointer_motion_absolute::<I>(event)
            }
            InputEvent::PointerButton { event } => self.on_pointer_button::<I>(event),
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<I>(event),
            _ => {}
        }
    }

    fn on_keyboard<I: InputBackend>(&mut self, event: I::KeyboardKeyEvent) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let keycode = event.key_code();
        let pressed = event.state() == KeyState::Pressed;
        let serial = SERIAL_COUNTER.next_serial();
        let time = Event::time_msec(&event);

        // The filter runs after xkb has applied this key, so `mods` already
        // reflects it: on Super's release, `logo` is false.
        let mut action = None;
        keyboard.input::<(), _>(
            self,
            keycode,
            event.state(),
            serial,
            time,
            |state, mods, handle| {
                let key = classify(handle.modified_sym());
                let mods = Mods {
                    logo: mods.logo,
                    alt: mods.alt,
                    ctrl: mods.ctrl,
                    shift: mods.shift,
                };
                let outcome = state.bindings.key(keycode.raw(), key, pressed, mods);
                action = outcome.action;
                if outcome.forward {
                    FilterResult::Forward
                } else {
                    FilterResult::Intercept(())
                }
            },
        );

        if let Some(action) = action {
            self.run_action(action);
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::Launch => self.launcher.toggle(&self.socket_name),
            Action::Snap {
                direction,
                first_in_hold,
            } => self.snap_focused(direction, first_in_hold),
            Action::SwitchNext => {
                if self.focus.step().is_some() {
                    self.sync_focus();
                }
            }
            Action::FinishSwitch => {
                if self.focus.is_switching() {
                    self.focus.finish_switch();
                    self.sync_focus();
                }
            }
            Action::SwitchVt(vt) => {
                if let Backend::Udev(udev) = &mut self.backend {
                    udev.change_vt(vt);
                }
            }
            Action::Quit => {
                info!("quit requested");
                self.loop_signal.stop();
            }
        }
    }

    fn snap_focused(&mut self, direction: Direction, first_in_hold: bool) {
        let Some(window) = self.focus.current().and_then(|id| self.window(id)) else {
            return;
        };
        let id = window.id;
        // Floating dialogs snap as if they were fullscreen.
        let current = window.layout.unwrap_or(Layout::Fullscreen);
        self.set_layout(id, current.after_direction(direction, first_in_hold));
    }

    /// Relative motion from a real mouse or touchpad (DRM backend).
    fn on_pointer_motion<I: InputBackend>(&mut self, event: I::PointerMotionEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let mut pos = pointer.current_location() + event.delta();

        // Keep the pointer on the screen.
        if let Some(geo) = self
            .primary_output()
            .and_then(|o| self.space.output_geometry(&o))
        {
            let max_x = (geo.loc.x + geo.size.w - 1) as f64;
            let max_y = (geo.loc.y + geo.size.h - 1) as f64;
            pos.x = pos.x.clamp(geo.loc.x as f64, max_x);
            pos.y = pos.y.clamp(geo.loc.y as f64, max_y);
        }

        self.move_pointer(pos, event.time_msec());
    }

    fn on_pointer_motion_absolute<I: InputBackend>(
        &mut self,
        event: I::PointerMotionAbsoluteEvent,
    ) {
        let Some(output) = self.primary_output() else {
            return;
        };
        let Some(output_geo) = self.space.output_geometry(&output) else {
            return;
        };
        let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
        self.move_pointer(pos, event.time_msec());
    }

    fn move_pointer(&mut self, pos: Point<f64, Logical>, time: u32) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let under = self.surface_under(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_button<I: InputBackend>(&mut self, event: I::PointerButtonEvent) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let state = event.state();

        if state == ButtonState::Pressed {
            self.bindings.pointer_button();
            if !pointer.is_grabbed()
                && let Some(id) = self.window_under(pointer.current_location())
            {
                self.focus_window(id);
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button: event.button_code(),
                state,
                serial,
                time: event.time_msec(),
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_axis<I: InputBackend>(&mut self, event: I::PointerAxisEvent) {
        let source = event.source();
        let mut frame = AxisFrame::new(event.time_msec()).source(source);

        for axis in [Axis::Horizontal, Axis::Vertical] {
            let discrete = event.amount_v120(axis);
            // Wheels report only discrete steps; 15 logical pixels per
            // notch matches what other compositors send.
            let amount = event
                .amount(axis)
                .unwrap_or_else(|| discrete.unwrap_or(0.0) * 15.0 / 120.0);
            if amount != 0.0 {
                frame = frame.value(axis, amount);
                if let Some(discrete) = discrete {
                    frame = frame.v120(axis, discrete as i32);
                }
            }
            if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                frame = frame.stop(axis);
            }
        }

        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        pointer.axis(self, frame);
        pointer.frame(self);
    }

    /// The window under `pos`, unless a top or overlay layer surface such as
    /// the launcher covers that spot.
    fn window_under(&self, pos: Point<f64, Logical>) -> Option<WindowId> {
        let output = self.space.output_under(pos).next()?;
        let output_geo = self.space.output_geometry(output)?;
        let layers = layer_map_for_output(output);
        let pos_in_output = pos - output_geo.loc.to_f64();
        if layers.layer_under(Layer::Overlay, pos_in_output).is_some()
            || layers.layer_under(Layer::Top, pos_in_output).is_some()
        {
            return None;
        }

        let (element, _) = self.space.element_under(pos)?;
        self.windows
            .iter()
            .find(|w| &w.window == element)
            .map(|w| w.id)
    }
}

fn classify(sym: Keysym) -> Key {
    match sym {
        Keysym::Super_L | Keysym::Super_R => Key::Super,
        Keysym::Alt_L | Keysym::Alt_R => Key::Alt,
        Keysym::Tab => Key::Tab,
        Keysym::Left => Key::Arrow(Direction::Left),
        Keysym::Right => Key::Arrow(Direction::Right),
        Keysym::Up => Key::Arrow(Direction::Up),
        Keysym::Down => Key::Arrow(Direction::Down),
        Keysym::BackSpace => Key::Backspace,
        sym if (Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw())
            .contains(&sym.raw()) =>
        {
            Key::SwitchVt((sym.raw() - Keysym::XF86_Switch_VT_1.raw() + 1) as i32)
        }
        _ => Key::Other,
    }
}
