//! ext-idle-notify and idle-inhibit: tell an idle daemon such as swayidle
//! when the user stops using the keyboard and mouse, unless an application
//! playing video asks to keep the screen on.
//!
//! ext-idle-notify is implemented here rather than with Smithay's helper,
//! which keeps the timeouts to itself: knowing when the next one fires is
//! what lets the screen fade to black just before it (see [`crate::fade`]).

use std::time::{Duration, Instant};

use smithay::{
    delegate_idle_inhibit,
    reexports::{
        calloop::{
            RegistrationToken,
            timer::{TimeoutAction, Timer},
        },
        wayland_protocols::ext::idle_notify::v1::server::{
            ext_idle_notification_v1::{self, ExtIdleNotificationV1},
            ext_idle_notifier_v1::{self, ExtIdleNotifierV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
            backend::ClientId, protocol::wl_surface::WlSurface,
        },
    },
    wayland::{compositor::get_parent, idle_inhibit::IdleInhibitHandler},
};
use tracing::info;

use crate::state::State;

/// One idle notification a client asked for.
#[derive(Debug)]
struct Notification {
    resource: ExtIdleNotificationV1,
    timeout: Duration,
    /// Asked for with `get_input_idle_notification`: fires even while an
    /// idle inhibitor is active.
    ignores_inhibit: bool,
    /// When it fires next, and the timer that fires it. `None` once it has
    /// fired, or while an inhibitor holds it.
    due: Option<(Instant, RegistrationToken)>,
    /// Told it is idle, and not yet that the user is back.
    idle: bool,
}

#[derive(Debug, Default)]
pub struct IdleNotifyState {
    notifications: Vec<Notification>,
    inhibited: bool,
    /// Starts the fade to black before the next notification fires.
    fade_timer: Option<RegistrationToken>,
}

impl IdleNotifyState {
    pub fn new(dh: &DisplayHandle) -> Self {
        dh.create_global::<State, ExtIdleNotifierV1, _>(2, ());
        Self::default()
    }
}

impl State {
    /// The user pressed a key or moved the pointer: every notification that
    /// had fired resumes, and all of them start counting again.
    pub fn idle_activity(&mut self) {
        for index in 0..self.idle_notify.notifications.len() {
            let notification = &mut self.idle_notify.notifications[index];
            if notification.idle {
                notification.idle = false;
                notification.resource.resumed();
            }
            self.start_idle_timer(index);
        }
        self.schedule_idle_fade();
    }

    fn start_idle_timer(&mut self, index: usize) {
        let inhibited = self.idle_notify.inhibited;
        let notification = &mut self.idle_notify.notifications[index];
        if let Some((_, token)) = notification.due.take() {
            self.loop_handle.remove(token);
        }
        if inhibited && !notification.ignores_inhibit {
            return;
        }
        let resource = notification.resource.clone();
        let timeout = notification.timeout;
        let timer = Timer::from_duration(timeout);
        let token = self.loop_handle.insert_source(timer, move |_, _, state| {
            state.idle_timeout(&resource);
            TimeoutAction::Drop
        });
        if let Ok(token) = token {
            notification.due = Some((Instant::now() + timeout, token));
        }
    }

    fn idle_timeout(&mut self, resource: &ExtIdleNotificationV1) {
        let Some(notification) = self
            .idle_notify
            .notifications
            .iter_mut()
            .find(|n| &n.resource == resource)
        else {
            return;
        };
        notification.due = None;
        if !notification.idle {
            notification.idle = true;
            info!(timeout = ?notification.timeout, "idle");
            notification.resource.idled();
        }
    }

    fn set_idle_inhibited(&mut self, inhibited: bool) {
        if self.idle_notify.inhibited == inhibited {
            return;
        }
        info!(inhibited, "idle inhibit changed");
        self.idle_notify.inhibited = inhibited;
        for index in 0..self.idle_notify.notifications.len() {
            let notification = &mut self.idle_notify.notifications[index];
            if notification.ignores_inhibit {
                continue;
            }
            if inhibited {
                if notification.idle {
                    notification.idle = false;
                    notification.resource.resumed();
                }
                if let Some((_, token)) = notification.due.take() {
                    self.loop_handle.remove(token);
                }
            } else {
                self.start_idle_timer(index);
            }
        }
        self.schedule_idle_fade();
    }

    /// Times the fade to black to end as the next idle notification fires.
    /// Only timeouts longer than the fade count, so a client asking to be
    /// told about a moment's idleness does not darken the screen.
    pub fn schedule_idle_fade(&mut self) {
        if let Some(token) = self.idle_notify.fade_timer.take() {
            self.loop_handle.remove(token);
        }
        let fade = Duration::from_millis(self.config.lock.fade_ms);
        if fade.is_zero() {
            return;
        }
        let Some(due) = self
            .idle_notify
            .notifications
            .iter()
            .filter(|n| n.timeout > fade)
            .filter_map(|n| n.due.map(|(due, _)| due))
            .min()
        else {
            return;
        };
        let timer = Timer::from_deadline(due - fade);
        self.idle_notify.fade_timer = self
            .loop_handle
            .insert_source(timer, |_, _, state| {
                state.idle_notify.fade_timer = None;
                state.start_idle_fade();
                TimeoutAction::Drop
            })
            .ok();
    }
}

impl GlobalDispatch<ExtIdleNotifierV1, ()> for State {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtIdleNotifierV1>,
        _data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ExtIdleNotifierV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        _notifier: &ExtIdleNotifierV1,
        request: ext_idle_notifier_v1::Request,
        _data: &(),
        dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (id, timeout, ignores_inhibit) = match request {
            ext_idle_notifier_v1::Request::GetIdleNotification { id, timeout, .. } => {
                (id, timeout, false)
            }
            ext_idle_notifier_v1::Request::GetInputIdleNotification { id, timeout, .. } => {
                (id, timeout, true)
            }
            _ => return,
        };
        let resource = data_init.init(id, ());
        let timeout = Duration::from_millis(timeout as u64);
        let pid = client.get_credentials(dh).map(|c| c.pid).unwrap_or(0);
        info!(
            pid,
            ?timeout,
            ignores_inhibit,
            "idle notification requested"
        );
        state.idle_notify.notifications.push(Notification {
            resource,
            timeout,
            ignores_inhibit,
            due: None,
            idle: false,
        });
        let index = state.idle_notify.notifications.len() - 1;
        state.start_idle_timer(index);
        state.schedule_idle_fade();
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &ExtIdleNotificationV1,
        _request: ext_idle_notification_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ExtIdleNotificationV1,
        _data: &(),
    ) {
        let Some(index) = state
            .idle_notify
            .notifications
            .iter()
            .position(|n| &n.resource == resource)
        else {
            return;
        };
        let notification = state.idle_notify.notifications.remove(index);
        if let Some((_, token)) = notification.due {
            state.loop_handle.remove(token);
        }
        state.schedule_idle_fade();
    }
}

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.push(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(index) = self.idle_inhibitors.iter().position(|s| s == &surface) {
            self.idle_inhibitors.swap_remove(index);
        }
    }
}
delegate_idle_inhibit!(State);

impl State {
    /// Keeps the session from going idle while a surface with an idle
    /// inhibitor is visible. Runs after every batch of events, since
    /// surfaces map, unmap and die at any time.
    pub fn update_idle_inhibit(&mut self) {
        self.idle_inhibitors.retain(|surface| surface.is_alive());
        let inhibited = self
            .idle_inhibitors
            .iter()
            .any(|surface| self.is_visible(surface));
        self.set_idle_inhibited(inhibited);
    }

    /// Whether `surface` belongs to something on screen: a mapped window or
    /// a layer surface, or while locked, only a lock surface.
    fn is_visible(&self, surface: &WlSurface) -> bool {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }
        if self.is_locked() {
            return self
                .space
                .outputs()
                .any(|output| self.lock_surface(output).map(|s| s.wl_surface()) == Some(&root));
        }
        self.window_for_surface(&root).is_some_and(|w| w.mapped)
            || self.layer_for_surface(&root).is_some()
    }
}
