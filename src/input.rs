//! Routes input events to keybindings and clients.

use std::time::Duration;

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
    reexports::calloop::timer::{TimeoutAction, Timer},
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::shell::wlr_layer::Layer,
};

use crate::{
    commands,
    focus::WindowId,
    keyboard::{Action, App, Command, Key, Mods},
    layout::{Direction, Layout},
    state::{Backend, State},
};
use tracing::{info, warn};

impl State {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        if !matches!(
            event,
            InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. }
        ) {
            // The user is here: the idle timers restart, the monitor wakes and
            // a dark screen lights up. A key's release says nothing new, so it
            // does not light the screen: after Super+l the lock stays dark
            // until the next press.
            self.idle_activity();
            self.power_on_outputs();
            let release = matches!(&event, InputEvent::Keyboard { event } if event.state() == KeyState::Released);
            if !release {
                self.wake_screen();
            }
        }
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

        // Any other key, or releasing the held one, stops a repeating key.
        if pressed
            || self
                .key_repeat
                .is_some_and(|(held, _)| held == keycode.raw())
        {
            self.stop_key_repeat();
        }

        // The filter runs after xkb has applied this key, so `mods` already
        // reflects it: on Super's release, `logo` is false.
        let mut action = None;
        let mut typed = None;
        keyboard.input::<(), _>(
            self,
            keycode,
            event.state(),
            serial,
            time,
            |state, mods, handle| {
                let key = match classify(handle.modified_sym()) {
                    // An empty command leaves the key to the client.
                    Key::Command(command) if state.config.commands.get(command).is_empty() => {
                        Key::Other
                    }
                    key if App::for_key(key, mods.shift)
                        .is_some_and(|app| state.config.commands.app(app).is_empty()) =>
                    {
                        Key::Other
                    }
                    key => key,
                };
                let mods = Mods {
                    logo: mods.logo,
                    alt: mods.alt,
                    ctrl: mods.ctrl,
                    shift: mods.shift,
                };
                state.bindings.switching = state.focus.is_switching();
                state.bindings.grouped = state
                    .focus
                    .current()
                    .is_some_and(|id| state.groups.group_of(id).is_some());
                state.bindings.locked = state.is_locked();
                state.bindings.overview = state.overview.open;
                state.bindings.overview_text = state.overview.has_text();
                let outcome = state.bindings.key(keycode.raw(), key, pressed, mods);
                action = outcome.action;
                if action == Some(Action::OverviewType) {
                    typed = handle.modified_sym().key_char();
                }
                if outcome.forward {
                    FilterResult::Forward
                } else {
                    FilterResult::Intercept(())
                }
            },
        );

        if let Some(action) = action {
            self.perform(action, typed);
            if action.repeats() {
                self.start_key_repeat(keycode.raw(), action, typed);
            }
        }
    }

    /// Runs `action`; `typed` is the character of a key typed into the
    /// overview's search.
    fn perform(&mut self, action: Action, typed: Option<char>) {
        match (action, typed) {
            (Action::OverviewType, Some(c)) => self.overview_type(c),
            (Action::OverviewType, None) => {}
            (action, _) => self.run_action(action),
        }
    }

    /// Runs `action` again at the key repeat rate while its key is held,
    /// after the usual repeat delay. Clients repeat keys themselves, but
    /// these never reach one.
    fn start_key_repeat(&mut self, keycode: u32, action: Action, typed: Option<char>) {
        let delay = Duration::from_millis(self.config.keyboard.repeat_delay as u64);
        let interval = Duration::from_millis(1000 / self.config.keyboard.repeat_rate as u64);
        let timer = Timer::from_duration(delay);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            state.perform(action, typed);
            TimeoutAction::ToDuration(interval)
        });
        match inserted {
            Ok(token) => self.key_repeat = Some((keycode, token)),
            Err(err) => warn!(%err, "failed to schedule key repeat"),
        }
    }

    /// Stops repeating a held key, if one repeats.
    pub fn stop_key_repeat(&mut self) {
        if let Some((_, token)) = self.key_repeat.take() {
            self.loop_handle.remove(token);
        }
    }

    fn run_action(&mut self, action: Action) {
        match action {
            Action::ToggleOverview => self.toggle_overview(),
            Action::CloseOverview => self.close_overview(),
            Action::OverviewActivate => self.overview_activate(),
            Action::OverviewMove(direction) => self.overview_move(direction),
            Action::OverviewEdit(edit) => self.overview_edit(edit),
            // Handled above, where the key's character is known.
            Action::OverviewType => {}
            Action::Snap {
                direction,
                first_in_hold,
            } => self.snap_focused(direction, first_in_hold),
            Action::Switch { forward } => self.step_switch(forward),
            Action::FinishSwitch => {
                if self.focus.is_switching() {
                    self.focus.finish_switch();
                    self.switcher.hide();
                    self.sync_focus();
                }
            }
            Action::CancelSwitch => {
                self.focus.cancel_switch();
                self.switcher.hide();
            }
            Action::SwitchVt(vt) => {
                if let Backend::Udev(udev) = &mut self.backend {
                    udev.change_vt(vt);
                }
            }
            Action::Run(command) => self.run_command(command),
            Action::Spawn(app) => {
                info!(?app, "starting from shortcut");
                commands::run(self.config.commands.app(app), &self.child_env(), || {});
            }
            Action::Close => {
                let focused = self.focus.current().and_then(|id| self.window(id));
                if let Some(toplevel) = focused.and_then(|w| w.window.toplevel()) {
                    toplevel.send_close();
                }
            }
            Action::FocusNth(n) => {
                // Dialogs are left out, so the numbers stay put when one opens.
                let nth = self
                    .windows
                    .iter()
                    .filter(|w| w.mapped)
                    .filter(|w| w.window.toplevel().is_some_and(|t| t.parent().is_none()))
                    .nth(n - 1)
                    .map(|w| w.id);
                if let Some(id) = nth {
                    self.focus_window(id);
                }
            }
            Action::Group => self.group_visible(),
            Action::Ungroup => self.ungroup_focused(),
            Action::CycleGroup { forward } => self.cycle_group(forward),
            Action::Quit => {
                info!("quit requested");
                self.loop_signal.stop();
            }
        }
    }

    /// Snaps the focused window. A group member snapped to full screen is zoomed: the first Super+arrow of a later hold puts it back in the tile it had when the zooming hold began.
    fn snap_focused(&mut self, direction: Direction, first_in_hold: bool) {
        let Some(window) = self.focus.current().and_then(|id| self.window(id)) else {
            return;
        };
        let (id, zoomed_from) = (window.id, window.unzoomed);
        // Floating dialogs snap as if they were fullscreen.
        let current = window.layout.unwrap_or(Layout::Fullscreen);
        let grouped = self.groups.group_of(id).is_some();
        if first_in_hold {
            self.snap_hold = Some((id, current));
        }
        let next = match zoomed_from {
            Some(tile) if grouped && first_in_hold && current == Layout::Fullscreen => tile,
            _ => current.after_direction(direction, first_in_hold),
        };
        let unzoomed = if grouped && next == Layout::Fullscreen {
            self.snap_hold
                .filter(|&(held, layout)| held == id && layout != Layout::Fullscreen)
                .map(|(_, layout)| layout)
                .or(zoomed_from)
        } else {
            None
        };
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == id) {
            window.unzoomed = unzoomed;
        }
        self.set_layout(id, next);
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
        self.panel_pointer_moved(pos);
        self.overview_pointer_moved(pos);
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
        let button = event.button_code();

        if state == ButtonState::Released && self.suppressed_buttons.remove(&button) {
            return;
        }
        if state == ButtonState::Pressed {
            // A new press means the release of any earlier swallowed press
            // was lost, such as when the mouse was unplugged mid-click.
            self.suppressed_buttons.remove(&button);
            self.bindings.pointer_button();
            if self.overview_click(pointer.current_location())
                || self.switcher_click(pointer.current_location())
            {
                self.suppressed_buttons.insert(button);
                return;
            }
            if !self.is_locked()
                && !pointer.is_grabbed()
                && let Some(id) = self.window_under(pointer.current_location())
            {
                if self.focus.current() != Some(id) {
                    info!(window = id.0, "window clicked");
                }
                self.focus_window(id);
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
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
    /// the panel covers that spot.
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
        Keysym::Tab | Keysym::ISO_Left_Tab => Key::Tab,
        Keysym::Escape => Key::Escape,
        Keysym::Left => Key::Arrow(Direction::Left),
        Keysym::Right => Key::Arrow(Direction::Right),
        Keysym::Up => Key::Arrow(Direction::Up),
        Keysym::Down => Key::Arrow(Direction::Down),
        Keysym::BackSpace => Key::Backspace,
        Keysym::Delete | Keysym::KP_Delete => Key::Delete,
        Keysym::Home | Keysym::KP_Home => Key::Home,
        Keysym::End | Keysym::KP_End => Key::End,
        Keysym::F4 => Key::F4,
        Keysym::Return | Keysym::KP_Enter => Key::Return,
        // Letters as lowercase, so Caps Lock makes no difference.
        sym if (Keysym::a.raw()..=Keysym::z.raw()).contains(&sym.raw()) => {
            Key::Char(sym.raw() as u8 as char)
        }
        sym if (Keysym::A.raw()..=Keysym::Z.raw()).contains(&sym.raw()) => {
            Key::Char((sym.raw() as u8 as char).to_ascii_lowercase())
        }
        sym if (Keysym::_1.raw()..=Keysym::_9.raw()).contains(&sym.raw()) => {
            Key::Digit((sym.raw() - Keysym::_0.raw()) as u8)
        }
        Keysym::XF86_MonBrightnessUp => Key::Command(Command::BrightnessUp),
        Keysym::XF86_MonBrightnessDown => Key::Command(Command::BrightnessDown),
        Keysym::XF86_AudioRaiseVolume => Key::Command(Command::VolumeUp),
        Keysym::XF86_AudioLowerVolume => Key::Command(Command::VolumeDown),
        Keysym::XF86_AudioMute => Key::Command(Command::VolumeMute),
        Keysym::XF86_AudioMicMute => Key::Command(Command::MicMute),
        Keysym::Print => Key::Command(Command::Screenshot),
        sym if (Keysym::XF86_Switch_VT_1.raw()..=Keysym::XF86_Switch_VT_12.raw())
            .contains(&sym.raw()) =>
        {
            Key::SwitchVt((sym.raw() - Keysym::XF86_Switch_VT_1.raw() + 1) as i32)
        }
        _ => Key::Other,
    }
}
