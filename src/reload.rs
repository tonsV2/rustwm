//! Applies the config file whenever it is saved, so settings can be tried without restarting rustwm. The file's directory is watched rather than the file, because many editors save by writing a new file and renaming it over the old one.

use std::{fs, io, os::fd::AsFd};

use inotify::{Inotify, WatchMask};
use smithay::{
    desktop::layer_map_for_output,
    output::Output,
    reexports::calloop::{Interest, Mode, PostAction, generic::Generic},
    wayland::{compositor::with_states, fractional_scale::with_fractional_scale},
};
use tracing::{info, warn};

use crate::{
    config::{self, Config},
    keymap,
    state::{State, xkb_config},
    udev,
};

impl State {
    pub fn watch_config(&mut self) {
        if let Err(err) = self.try_watch_config() {
            warn!(%err, "cannot watch the config file; changes apply after a restart");
        }
    }

    fn try_watch_config(&mut self) -> io::Result<()> {
        let Some(path) = config::path() else {
            return Ok(());
        };
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return Ok(());
        };
        fs::create_dir_all(dir)?;
        let mut inotify = Inotify::init()?;
        inotify
            .watches()
            .add(dir, WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO)?;
        let fd = inotify.as_fd().try_clone_to_owned()?;
        let name = name.to_owned();
        let mut buffer = [0; 4096];
        self.loop_handle
            .insert_source(
                Generic::new(fd, Interest::READ, Mode::Level),
                move |_, _, state| {
                    let mut changed = false;
                    loop {
                        match inotify.read_events(&mut buffer) {
                            Ok(events) => {
                                changed |= events.into_iter().any(|e| e.name == Some(&name))
                            }
                            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                            Err(err) => return Err(err),
                        }
                    }
                    if changed {
                        state.reload_config();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|err| io::Error::other(err.error))?;
        Ok(())
    }

    fn reload_config(&mut self) {
        let Some(config) = Config::read() else {
            warn!("keeping the current settings");
            return;
        };
        if config == self.config {
            return;
        }
        let old = std::mem::replace(&mut self.config, config);
        if old.input != self.config.input {
            udev::reconfigure_input(self);
        }
        if old.keyboard != self.config.keyboard {
            self.reconfigure_keyboard();
        }
        if old.output != self.config.output {
            udev::rescale_output(self);
        }
        if old.panel.command != self.config.panel.command
            || old.panel.position != self.config.panel.position
            || old.panel.mode != self.config.panel.mode
        {
            info!("panel command, position and mode apply after a restart");
        }
    }

    fn reconfigure_keyboard(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let config = &self.config.keyboard;
        keyboard.change_repeat_info(config.repeat_rate, config.repeat_delay);
        let names = keymap::resolve(config);
        match keyboard.set_xkb_config(self, xkb_config(&names)) {
            Ok(()) => info!(?names, "keyboard layout set"),
            Err(err) => warn!(
                ?names,
                ?err,
                "invalid keyboard layout; keeping the current one"
            ),
        }
    }

    /// Tells every surface on `output` its new scale and lays windows and layer surfaces out again for the new logical size.
    pub fn output_rescaled(&mut self, output: &Output) {
        let scale = output.current_scale().fractional_scale();
        let set_scale = |_: &_, states: &_| {
            with_fractional_scale(states, |fractional| fractional.set_preferred_scale(scale));
        };
        for managed in &self.windows {
            managed.window.with_surfaces(set_scale);
        }
        let layers = layer_map_for_output(output)
            .layers()
            .cloned()
            .collect::<Vec<_>>();
        for layer in layers {
            layer.with_surfaces(set_scale);
        }
        if let Some(lock) = self.lock_surface(output) {
            with_states(lock.wl_surface(), |states| {
                set_scale(lock.wl_surface(), states)
            });
        }
        layer_map_for_output(output).arrange();
        self.relayout_output(output);
    }
}
