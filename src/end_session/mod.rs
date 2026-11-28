//! Leaving the session from the overview's search, as in GNOME. Suspend
//! asks logind at once. Log Out, Restart and Power Off show a dialog in the
//! middle of the screen that counts down from 60 seconds; confirming asks
//! every window to close, so applications can save, and acts once they are
//! gone. Windows still open after a few seconds are listed in a panel at
//! the bottom, out of the way of their save prompts, with an Anyway button.
//! Restart and Power Off ask logind first whether they are allowed, before
//! any window is closed, and exit rustwm only once logind has accepted.
//!
//! The state machine is in `machine.rs` and the drawing in `draw.rs`; this
//! file connects them to the compositor: timers, logind, windows, input
//! and render elements.

mod draw;
mod machine;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use fontdue::Font;
use resvg::tiny_skia::Pixmap;
use smithay::{
    backend::allocator::Fourcc,
    backend::renderer::{
        element::{
            Id, Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            solid::SolidColorRenderElement,
        },
        utils::CommitCounter,
    },
    desktop::layer_map_for_output,
    output::Output,
    reexports::calloop::{
        RegistrationToken,
        timer::{TimeoutAction, Timer},
    },
    utils::{Logical, Point, Rectangle, Transform},
};
use tracing::{debug, info, warn};

pub use machine::{EndAction, Interruption};

use self::{
    draw::{ButtonView, Hit, Layout, Placement, Row, View},
    machine::{Machine, Phase, Step},
};
use crate::{
    apps::{self, AppInfo},
    logind::{Availability, Failure, PowerAction, Reply, Request},
    state::State,
    switcher::{SwitcherElement, app_id_and_title, image_src, load_font, text_width},
    system::{self, SystemAction},
};

/// How long the windows get to close before the panel listing those still
/// open shows.
const CLOSE_GRACE: Duration = Duration::from_secs(3);
/// How long to wait for logind to act before giving up on it: longer than
/// busctl's own timeout, so this only fires if busctl hangs.
const ACT_TIMEOUT: Duration = Duration::from_secs(130);
/// How long an availability question may go unanswered before another
/// may be sent: longer than busctl's timeouts for its three questions.
const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(20);
/// How long after a screen turns on the dialog takes no clicks: monitors
/// stay black for a second or more while they wake.
const WAKE_GRACE: Duration = Duration::from_secs(2);
/// The most applications listed by name; the rest are counted.
const MAX_ROWS: usize = 5;
/// The dimming over the desktop behind the dialog.
const DIM: [f32; 4] = [0.0, 0.0, 0.0, 0.5];

/// A key the dialog takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKey {
    Move { forward: bool },
    Press,
    Cancel,
}

/// Where the panel was last drawn, for the pointer's hit tests.
#[derive(Debug)]
struct Drawn {
    /// The output's position, so pointer positions can be made relative.
    output: Point<i32, Logical>,
    layout: Layout,
}

pub struct EndSession {
    pub machine: Machine,
    /// Nested, Suspend, Restart and Power Off would act on the host, so
    /// only Log Out is offered.
    nested: bool,
    /// What logind last said it can do.
    pub availability: Availability,
    /// Keeps the availability questions and answers in order.
    availability_queries: AvailabilityQueries,
    /// Numbers the dialogs. Bumped when one opens, is cancelled or is
    /// replaced, it tags logind requests and timers, so a reply or timer
    /// meant for an earlier dialog is dropped.
    ticket: u64,
    countdown: Option<RegistrationToken>,
    grace: Option<RegistrationToken>,
    watchdog: Option<RegistrationToken>,
    dim_id: Id,
    dim_commit: CommitCounter,
    /// Loaded on first use; the inner `None` means no font was found.
    font: Option<Option<Font>>,
    app_infos: HashMap<String, AppInfo>,
    /// Icons by name and pixel size; `None` when the icon was not found.
    icons: HashMap<(String, u32), Option<Pixmap>>,
    /// The panel last drawn, for this view and scale.
    cache: Option<((View, u64), MemoryRenderBuffer)>,
    drawn: Option<Drawn>,
    /// When a screen was last turned back on. A monitor stays black for a
    /// second or more after that though frames are drawn, so the dialog's
    /// buttons do not take clicks for [`WAKE_GRACE`].
    powered_on_at: Option<Instant>,
}

impl EndSession {
    pub fn new(nested: bool) -> Self {
        Self {
            machine: Machine::default(),
            nested,
            availability: Availability::default(),
            availability_queries: AvailabilityQueries::default(),
            ticket: 0,
            countdown: None,
            grace: None,
            watchdog: None,
            dim_id: Id::new(),
            dim_commit: CommitCounter::default(),
            font: None,
            app_infos: HashMap::new(),
            icons: HashMap::new(),
            cache: None,
            drawn: None,
            powered_on_at: None,
        }
    }

    /// Whether the dialog takes all input, as the overview does.
    pub fn is_modal(&self) -> bool {
        self.machine.is_modal()
    }

    /// Whether `pos` is on the panel listing the windows still open, which
    /// is not modal, so only it is kept from the windows under it.
    pub fn covers(&self, pos: Point<f64, Logical>) -> bool {
        matches!(self.machine.phase, Phase::Blocked { .. })
            && self
                .drawn
                .as_ref()
                .is_some_and(|drawn| drawn.layout.hit(pos - drawn.output.to_f64()) != Hit::Outside)
    }

    /// The screen was turned back on at `now`.
    pub fn screen_woke(&mut self, now: Instant) {
        self.powered_on_at = Some(now);
    }

    /// Forgets where the panel was drawn, such as when the screen turned
    /// off: nothing of it can be hit until the next frame draws it again.
    pub fn forget_drawn(&mut self) {
        self.drawn = None;
    }

    /// What is under `pos` on the panel last drawn; `None` before the
    /// first frame.
    fn hit(&self, pos: Point<f64, Logical>) -> Option<Hit> {
        let drawn = self.drawn.as_ref()?;
        Some(drawn.layout.hit(pos - drawn.output.to_f64()))
    }

    fn app_info(&mut self, app_id: &str) -> AppInfo {
        self.app_infos
            .entry(app_id.to_string())
            .or_insert_with(|| apps::app_info(app_id))
            .clone()
    }

    /// The panel's image for `view`, drawn at `scale`, or the last drawing.
    fn image(&mut self, view: View, layout: &Layout, scale: f64) -> &MemoryRenderBuffer {
        let key = (view, scale.to_bits());
        if self.cache.as_ref().is_none_or(|(cached, _)| *cached != key) {
            let icon_px = (draw::ROW_ICON as f64 * scale).round() as u32;
            for row in &key.0.rows {
                if let Some(icon) = &row.icon {
                    self.icons
                        .entry((icon.clone(), icon_px))
                        .or_insert_with(|| apps::load_icon(icon, icon_px));
                }
            }
            let icons: Vec<Option<&Pixmap>> = key
                .0
                .rows
                .iter()
                .map(|row| {
                    let icon = row.icon.as_ref()?;
                    self.icons.get(&(icon.clone(), icon_px))?.as_ref()
                })
                .collect();
            let font = self.font.get_or_insert_with(load_font).as_ref();
            let image = draw::draw(font, &key.0, layout, &icons, scale);
            let buffer = MemoryRenderBuffer::from_slice(
                &image.pixels,
                Fourcc::Argb8888,
                (image.width, image.height),
                1,
                Transform::Normal,
                None,
            );
            self.cache = Some((key, buffer));
        }
        &self.cache.as_ref().expect("cache was just filled").1
    }
}

impl State {
    /// Asks logind again which actions it can do, as at startup and each
    /// time the overview opens, so a change such as a new policy shows up
    /// in the search.
    pub fn refresh_system_actions(&mut self) {
        if self.end_session.nested {
            return;
        }
        if let Some(seq) = self.end_session.availability_queries.ask(Instant::now()) {
            self.logind.request(Request::Availability { seq });
        }
    }

    /// The actions the overview's search can find.
    pub fn offered_system_actions(&self) -> Vec<SystemAction> {
        system::offered(self.end_session.availability, self.end_session.nested)
    }

    /// Runs `action`, picked in the overview: Suspend at once, the others
    /// through the dialog, which replaces one already showing or waiting
    /// for windows.
    pub fn start_system_action(&mut self, action: SystemAction) {
        if self.is_locked() {
            return;
        }
        if !self.offered_system_actions().contains(&action) {
            // logind's answer changed since the search showed it.
            info!(?action, "no longer offered; ignored");
            return;
        }
        if let Phase::Acting { action: acting } = self.end_session.machine.phase {
            info!(?action, ?acting, "already waiting for logind; ignored");
            return;
        }
        match action.end_action() {
            None => {
                info!("suspending");
                self.logind.request(Request::Perform {
                    ticket: self.end_session.ticket,
                    power: PowerAction::Suspend,
                });
            }
            Some(end_action) => self.open_end_session(end_action),
        }
    }

    fn open_end_session(&mut self, action: EndAction) {
        info!(?action, "end session dialog opened");
        self.forget_end_session();
        // As when the overview opens: no key repeats into the dialog, an
        // Alt+Tab switch ends with nothing changed, and a popup menu or a
        // held button lets go of its window.
        self.stop_key_repeat();
        self.focus.cancel_switch();
        self.switcher.hide();
        self.close_overview();
        self.release_grabs();
        let availability = self.end_session.availability;
        self.end_session
            .machine
            .open(action, availability, Instant::now());
        self.start_countdown();
        if action.power().is_some() {
            self.logind.request(Request::Inhibitors {
                ticket: self.end_session.ticket,
            });
        }
        // The keyboard goes to nobody while the dialog shows.
        self.sync_focus();
        self.refresh_pointer_focus();
    }

    /// Stops the timers and makes replies to earlier requests stale, for a
    /// dialog that went away or is replaced.
    fn forget_end_session(&mut self) {
        self.stop_end_session_timers();
        self.end_session.ticket += 1;
        self.end_session.drawn = None;
        self.end_session.cache = None;
    }

    fn stop_end_session_timers(&mut self) {
        let end_session = &mut self.end_session;
        for token in [
            end_session.countdown.take(),
            end_session.grace.take(),
            end_session.watchdog.take(),
        ]
        .into_iter()
        .flatten()
        {
            self.loop_handle.remove(token);
        }
    }

    /// A key while the dialog is modal.
    pub fn end_session_key(&mut self, key: DialogKey) {
        if !self.end_session.is_modal() {
            return;
        }
        match key {
            DialogKey::Move { forward } => self.end_session.machine.move_selection(forward),
            DialogKey::Press => {
                let step = self.end_session.machine.press_selected(Instant::now());
                self.run_step(step);
            }
            DialogKey::Cancel => {
                let step = self.end_session.machine.cancel();
                self.run_step(step);
            }
        }
    }

    /// Selects the button under the pointer.
    pub fn end_session_pointer_moved(&mut self, pos: Point<f64, Logical>) {
        if let Some(Hit::Button(index)) = self.end_session.hit(pos) {
            self.end_session.machine.select(index);
        }
    }

    /// A click on the dialog presses the button under it, and one outside
    /// a modal dialog cancels it. Returns whether the click was the
    /// dialog's: while it is modal, every click is.
    pub fn end_session_click(&mut self, pos: Point<f64, Logical>) -> bool {
        let modal = self.end_session.is_modal();
        if !modal && !self.end_session.covers(pos) {
            return false;
        }
        // Still dark to the eye: the click is taken, but presses nothing.
        if just_woke(self.end_session.powered_on_at, Instant::now()) {
            info!("click on the end session dialog ignored right after the screen woke");
            return true;
        }
        let step = match self.end_session.hit(pos) {
            Some(Hit::Button(index)) => self.end_session.machine.press(index, Instant::now()),
            Some(Hit::Outside) if modal => self.end_session.machine.cancel(),
            // Before the first frame the click cannot have been aimed at
            // anything, so it neither presses nor cancels.
            _ => Step::Nothing,
        };
        self.run_step(step);
        true
    }

    /// The user can no longer see the dialog, for `why`; see
    /// [`Machine::interrupt`] for what goes away.
    pub fn end_session_interrupted(&mut self, why: Interruption) {
        if self.end_session.machine.interrupt(why) {
            info!(?why, "end session cancelled");
            self.forget_end_session();
            // As for any cancel: the keyboard goes back to the windows.
            self.sync_focus();
            self.refresh_pointer_focus();
        }
    }

    /// Runs after every batch of events, which is where a window that
    /// closed, crashed or unmapped shows up: once the last one is gone, the
    /// action goes ahead.
    pub fn end_session_after_dispatch(&mut self) {
        if self.end_session.machine.waiting_for_windows() {
            let count = self.mapped_window_count();
            let step = self.end_session.machine.windows_left(count);
            self.run_step(step);
        }
    }

    fn mapped_window_count(&self) -> usize {
        self.windows.iter().filter(|w| w.mapped).count()
    }

    /// Does what the state machine asks for.
    fn run_step(&mut self, step: Step) {
        match step {
            Step::Nothing => {}
            Step::Cancelled => {
                info!("end session cancelled");
                self.forget_end_session();
                self.sync_focus();
                self.refresh_pointer_focus();
            }
            Step::Check(action) => {
                let Some(power) = action.power() else {
                    return self.run_step(Step::CloseWindows(action));
                };
                if self.end_session.nested {
                    return self.refuse_nested(action);
                }
                // The countdown stops while logind is asked.
                if let Some(token) = self.end_session.countdown.take() {
                    self.loop_handle.remove(token);
                }
                self.logind.request(Request::Check {
                    ticket: self.end_session.ticket,
                    power,
                });
            }
            Step::CloseWindows(action) => self.close_windows_for(action),
            Step::Perform(action) => {
                self.stop_end_session_timers();
                match action.power() {
                    None => {
                        info!("logging out");
                        // Final, so the next batch of events does not
                        // log out again while the loop winds down.
                        self.end_session.machine.acting(action);
                        self.loop_signal.stop();
                    }
                    Some(_) if self.end_session.nested => self.refuse_nested(action),
                    Some(power) => {
                        info!(?action, "asking logind");
                        self.end_session.machine.acting(action);
                        self.end_session.drawn = None;
                        self.logind.request(Request::Perform {
                            ticket: self.end_session.ticket,
                            power,
                        });
                        self.start_watchdog();
                    }
                }
            }
        }
    }

    /// Nested, logind is the host's, so Restart and Power Off are refused
    /// before anything is closed or asked, whatever opened the dialog.
    fn refuse_nested(&mut self, action: EndAction) {
        self.show_end_session_error(
            action.failure_title().to_string(),
            "Not available in a nested session.".to_string(),
        );
    }

    /// Asks every window to close, as Super+q does, so applications can
    /// save, and waits for them.
    fn close_windows_for(&mut self, action: EndAction) {
        info!(?action, "closing every window");
        self.stop_end_session_timers();
        self.end_session.machine.start_closing(action);
        self.end_session.drawn = None;
        // Unmapped ones too, though only mapped ones are waited for, so a
        // hidden toplevel that never closes does not hold the action up.
        let ids: Vec<_> = self.windows.iter().map(|w| w.id).collect();
        for id in ids {
            self.close_window(id);
        }
        // The keyboard comes back, so save prompts can be answered.
        self.sync_focus();
        self.refresh_pointer_focus();
        self.start_grace();
        let count = self.mapped_window_count();
        let step = self.end_session.machine.windows_left(count);
        self.run_step(step);
    }

    /// Shows the error dialog.
    fn show_end_session_error(&mut self, title: String, message: String) {
        warn!(title, message, "end session failed");
        self.stop_end_session_timers();
        self.stop_key_repeat();
        self.close_overview();
        self.release_grabs();
        self.end_session.machine.failed(title, message);
        self.end_session.drawn = None;
        self.sync_focus();
        self.refresh_pointer_focus();
    }

    /// Schedules the next tick of the countdown, at the next whole second.
    fn start_countdown(&mut self) {
        let Phase::Confirm { deadline, .. } = self.end_session.machine.phase else {
            return;
        };
        let ticket = self.end_session.ticket;
        let timer = Timer::from_duration(machine::until_next_tick(deadline, Instant::now()));
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            // Cleared first, so nothing removes this timer from inside its
            // own callback.
            state.end_session.countdown = None;
            if state.end_session.ticket != ticket {
                return TimeoutAction::Drop;
            }
            let step = state.end_session.machine.tick(Instant::now());
            state.run_step(step);
            // A callback does not know its own token, so the next tick is
            // a new timer rather than this one rescheduled.
            if state.end_session.ticket == ticket && state.end_session.machine.is_counting() {
                state.start_countdown();
            }
            TimeoutAction::Drop
        });
        match inserted {
            Ok(token) => self.end_session.countdown = Some(token),
            // The dialog still works; only the automatic confirm is lost.
            Err(err) => warn!(%err, "failed to schedule the countdown"),
        }
    }

    /// After the grace time, lists the windows still open.
    fn start_grace(&mut self) {
        let ticket = self.end_session.ticket;
        let timer = Timer::from_duration(CLOSE_GRACE);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            state.end_session.grace = None;
            if state.end_session.ticket == ticket {
                let count = state.mapped_window_count();
                state.end_session.machine.grace_over(count);
                // The panel may now be under the pointer.
                state.refresh_pointer_focus();
            }
            TimeoutAction::Drop
        });
        match inserted {
            Ok(token) => self.end_session.grace = Some(token),
            Err(err) => {
                // Without the timer, the panel shows at once.
                warn!(%err, "failed to schedule the grace time");
                let count = self.mapped_window_count();
                self.end_session.machine.grace_over(count);
            }
        }
    }

    /// Gives up on logind if it never answers, so the session does not sit
    /// waiting forever.
    fn start_watchdog(&mut self) {
        let ticket = self.end_session.ticket;
        let timer = Timer::from_duration(ACT_TIMEOUT);
        let inserted = self.loop_handle.insert_source(timer, move |_, _, state| {
            state.end_session.watchdog = None;
            if let Phase::Acting { action } = state.end_session.machine.phase
                && state.end_session.ticket == ticket
            {
                state.logind_failed(action, Failure::NoAnswer);
            }
            TimeoutAction::Drop
        });
        match inserted {
            Ok(token) => self.end_session.watchdog = Some(token),
            Err(err) => warn!(%err, "failed to schedule waiting for logind"),
        }
    }

    pub fn logind_reply(&mut self, reply: Reply) {
        let current = self.end_session.ticket;
        match reply {
            Reply::Availability { seq, availability } => {
                let newest = self
                    .end_session
                    .availability_queries
                    .answered(seq, availability.is_some());
                if let Some(availability) = availability.filter(|_| newest) {
                    if availability != self.end_session.availability {
                        info!(?availability, "logind's actions changed");
                    }
                    self.end_session.availability = availability;
                }
            }
            Reply::Inhibitors { ticket, inhibitors } if ticket == current => {
                self.end_session.machine.set_inhibitors(inhibitors);
            }
            Reply::Checked {
                ticket,
                power,
                result,
            } if ticket == current => {
                let Some(action) = end_action_of(power) else {
                    return;
                };
                if let Err(message) = &result {
                    warn!(?action, message, "logind does not allow it");
                }
                let step = self.end_session.machine.checked(action, result);
                self.run_step(step);
            }
            Reply::Done {
                power: PowerAction::Suspend,
                result,
                ..
            } => match result {
                Ok(()) => debug!("logind suspended"),
                // Shown only when nothing else is: a dialog opened since
                // stays, and the lock before sleeping hides it anyway.
                Err(failure)
                    if self.end_session.machine.phase == Phase::Idle && !self.is_locked() =>
                {
                    self.show_end_session_error("Could not suspend".to_string(), failure.message());
                }
                Err(failure) => warn!(?failure, "logind did not suspend"),
            },
            Reply::Done {
                ticket,
                power,
                result,
            } if ticket == current => {
                let Phase::Acting { action } = self.end_session.machine.phase else {
                    return;
                };
                match result {
                    Ok(()) => {
                        info!(?power, "logind accepted; exiting");
                        self.loop_signal.stop();
                    }
                    Err(failure) => self.logind_failed(action, failure),
                }
            }
            reply => debug!(?reply, "stale logind reply dropped"),
        }
    }

    /// logind refused `action` or did not answer, after the windows were
    /// closed: rustwm stays, and says why unless the screen is locked.
    fn logind_failed(&mut self, action: EndAction, failure: Failure) {
        self.stop_end_session_timers();
        if self.is_locked() {
            warn!(?action, ?failure, "logind refused while locked");
            self.end_session.machine.reset();
            return;
        }
        // Without an answer logind may still be acting, so the title must
        // not say it failed.
        let title = match failure {
            Failure::NoAnswer => "No answer from logind".to_string(),
            Failure::Refused(_) => action.failure_title().to_string(),
        };
        self.show_end_session_error(title, failure.message());
    }

    /// The dialog for `output`, front to back: the panel, then the dimming
    /// behind a modal one. Empty when nothing is shown.
    pub fn end_session_elements(&mut self, output: &Output) -> Vec<SwitcherElement> {
        let placement = match self.end_session.machine.phase {
            _ if self.is_locked() => None,
            Phase::Confirm { .. } | Phase::Error { .. } => Some(Placement::Centre),
            // The overview covers the panel; closing it brings it back.
            Phase::Blocked { .. } if !self.overview.open => Some(Placement::Bottom),
            _ => None,
        };
        let geometry = self.space.output_geometry(output);
        let (Some(placement), Some(geometry)) = (placement, geometry) else {
            self.end_session.drawn = None;
            return Vec::new();
        };
        let area = match placement {
            Placement::Centre => Rectangle::from_size(geometry.size),
            Placement::Bottom => layer_map_for_output(output).non_exclusive_zone(),
        };
        let scale = output.current_scale().fractional_scale();
        let view = self.end_session_view(placement);
        let layout = draw::layout(&view, area);
        // Whole physical pixels, so the image is not blurred.
        let origin = layout
            .panel
            .loc
            .to_f64()
            .to_physical(scale)
            .to_i32_round::<i32>();
        let size = layout.panel.size;
        let buffer = self.end_session.image(view, &layout, scale);
        let panel = MemoryRenderBufferRenderElement::from_buffer(
            self.backend.renderer(),
            origin.to_f64(),
            buffer,
            None,
            Some(image_src(size, scale)),
            Some(size),
            Kind::Unspecified,
        );
        self.end_session.drawn = Some(Drawn {
            output: geometry.loc,
            layout,
        });
        let mut elements = Vec::new();
        match panel {
            Ok(panel) => elements.push(SwitcherElement::Image(panel)),
            Err(err) => warn!(?err, "failed to draw the end session dialog"),
        }
        // The dimming is a flat colour the GPU draws, rather than part of a
        // screen-sized image.
        if placement == Placement::Centre
            && let Some(mode) = output.current_mode()
        {
            elements.push(SwitcherElement::Solid(SolidColorRenderElement::new(
                self.end_session.dim_id.clone(),
                Rectangle::from_size(mode.size),
                self.end_session.dim_commit,
                DIM,
                Kind::Unspecified,
            )));
        }
        elements
    }

    /// What the panel shows now.
    fn end_session_view(&mut self, placement: Placement) -> View {
        let machine = &self.end_session.machine;
        let buttons = machine.buttons();
        let count = buttons.len();
        let buttons = buttons
            .into_iter()
            .enumerate()
            .map(|(index, button)| ButtonView {
                label: button.label(),
                // Only the last, confirming button is highlighted; Power
                // Off's Restart is an ordinary button, as in GNOME.
                suggested: button.goes_ahead() && index + 1 == count,
            })
            .collect();
        let selected = machine.selected();
        let seconds = machine.seconds_left(Instant::now());
        let (title, paragraphs, rows) = match &machine.phase {
            Phase::Confirm {
                action, inhibitors, ..
            } => {
                let mut paragraphs = vec![action.countdown_text(seconds.unwrap_or(0))];
                if !inhibitors.is_empty() {
                    paragraphs.push("Some applications are busy or have unsaved work.".into());
                }
                let rows = inhibitors
                    .iter()
                    .map(|inhibitor| Row {
                        name: inhibitor.who.clone(),
                        detail: inhibitor.why.clone(),
                        icon: None,
                    })
                    .collect();
                (action.title().to_string(), paragraphs, rows)
            }
            Phase::Blocked { action, .. } => {
                let title = action.title().to_string();
                let paragraphs = vec!["Some applications are still running.".to_string()];
                (title, paragraphs, self.running_apps())
            }
            Phase::Error { title, message } => (title.clone(), vec![message.clone()], Vec::new()),
            Phase::Idle | Phase::Closing { .. } | Phase::Acting { .. } => {
                (String::new(), Vec::new(), Vec::new())
            }
        };
        let more = rows.len().saturating_sub(MAX_ROWS);
        let rows = rows.into_iter().take(MAX_ROWS).collect();
        View {
            title,
            lines: self.wrap(&paragraphs),
            rows,
            more,
            buttons,
            selected,
            placement,
        }
    }

    /// The applications with a window still open, each once.
    fn running_apps(&mut self) -> Vec<Row> {
        let app_ids: Vec<String> = self
            .windows
            .iter()
            .filter(|w| w.mapped)
            .filter_map(|w| w.wl_surface())
            .map(|surface| app_id_and_title(surface).0)
            .collect();
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        for app_id in app_ids {
            let app = self.end_session.app_info(&app_id);
            let name = if app.name.is_empty() {
                "Unknown application".to_string()
            } else {
                app.name
            };
            if seen.insert(name.clone()) {
                rows.push(Row {
                    name,
                    detail: String::new(),
                    icon: app.icon,
                });
            }
        }
        rows
    }

    /// `paragraphs` broken into lines that fit the panel.
    fn wrap(&mut self, paragraphs: &[String]) -> Vec<String> {
        let font = self.end_session.font.get_or_insert_with(load_font).as_ref();
        let Some(font) = font else {
            return paragraphs.to_vec();
        };
        paragraphs
            .iter()
            .flat_map(|paragraph| {
                crate::switcher::wrap_lines(paragraph, draw::TEXT_WIDTH as f32, |text| {
                    text_width(font, text, draw::TEXT_SIZE)
                })
            })
            .collect()
    }
}

/// Numbers the availability questions sent to logind, so that at most one
/// is on its way, one that got lost does not stop the asking, and a late
/// answer never replaces a newer one.
#[derive(Debug, Default)]
struct AvailabilityQueries {
    /// The number of the latest question sent.
    sent: u64,
    /// The number of the question whose answer is in use.
    applied: u64,
    /// The question on its way, and when it was sent. After
    /// [`AVAILABILITY_TIMEOUT`] it counts as lost.
    in_flight: Option<(u64, Instant)>,
}

impl AvailabilityQueries {
    /// The number for a new question at `now`, or `None` while one is
    /// still on its way.
    fn ask(&mut self, now: Instant) -> Option<u64> {
        let waiting = self
            .in_flight
            .is_some_and(|(_, asked)| now.saturating_duration_since(asked) < AVAILABILITY_TIMEOUT);
        if waiting {
            return None;
        }
        self.sent += 1;
        self.in_flight = Some((self.sent, now));
        Some(self.sent)
    }

    /// Question `seq` was answered, with an answer if `ok`. Returns whether
    /// that answer is newer than the one in use, and so replaces it.
    fn answered(&mut self, seq: u64, ok: bool) -> bool {
        if self.in_flight.is_some_and(|(asked, _)| asked == seq) {
            self.in_flight = None;
        }
        if ok && seq > self.applied {
            self.applied = seq;
            true
        } else {
            false
        }
    }
}

/// Whether a screen turned on at `powered_on_at` may still look black at
/// `now`.
fn just_woke(powered_on_at: Option<Instant>, now: Instant) -> bool {
    powered_on_at.is_some_and(|on| now.saturating_duration_since(on) < WAKE_GRACE)
}

/// The dialog's action for what logind was asked to do.
fn end_action_of(power: PowerAction) -> Option<EndAction> {
    match power {
        PowerAction::Suspend => None,
        PowerAction::Reboot => Some(EndAction::Restart),
        PowerAction::PowerOff => Some(EndAction::PowerOff),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_hit_once_the_drawing_is_forgotten() {
        let mut end_session = EndSession::new(false);
        end_session.machine.start_closing(EndAction::PowerOff);
        end_session.machine.grace_over(1);
        let view = View {
            title: "Power Off".into(),
            lines: Vec::new(),
            rows: Vec::new(),
            more: 0,
            buttons: vec![
                ButtonView {
                    label: "Cancel".into(),
                    suggested: false,
                },
                ButtonView {
                    label: "Power Off Anyway".into(),
                    suggested: true,
                },
            ],
            selected: 1,
            placement: Placement::Bottom,
        };
        let layout = draw::layout(&view, Rectangle::from_size((1280, 800).into()));
        let anyway = layout.buttons[1];
        let pos = (anyway.loc + anyway.size.downscale(2).to_point()).to_f64();
        end_session.drawn = Some(Drawn {
            output: (0, 0).into(),
            layout,
        });
        assert_eq!(end_session.hit(pos), Some(Hit::Button(1)));
        assert!(end_session.covers(pos));

        end_session.forget_drawn();
        assert_eq!(end_session.hit(pos), None);
        assert!(!end_session.covers(pos));
        // The wait for windows itself goes on.
        assert!(end_session.machine.waiting_for_windows());
    }

    #[test]
    fn clicks_wait_until_the_screen_has_woken() {
        let now = Instant::now();
        assert!(!just_woke(None, now));
        assert!(just_woke(Some(now), now));
        assert!(just_woke(Some(now), now + Duration::from_millis(1999)));
        assert!(!just_woke(Some(now), now + WAKE_GRACE));
        // A clock that seems to go back counts as just woken.
        assert!(just_woke(Some(now + Duration::from_secs(1)), now));
    }

    #[test]
    fn one_availability_question_at_a_time() {
        let now = Instant::now();
        let mut queries = AvailabilityQueries::default();
        assert_eq!(queries.ask(now), Some(1));
        assert_eq!(queries.ask(now + Duration::from_secs(1)), None);
        // An unanswered question counts as lost after a while.
        assert_eq!(queries.ask(now + AVAILABILITY_TIMEOUT), Some(2));
    }

    #[test]
    fn a_late_availability_answer_does_not_replace_a_newer_one() {
        let now = Instant::now();
        let later = now + AVAILABILITY_TIMEOUT;
        let mut queries = AvailabilityQueries::default();
        queries.ask(now);
        queries.ask(later);
        // The second question is answered first and applied.
        assert!(queries.answered(2, true));
        // The first one's late answer is dropped.
        assert!(!queries.answered(1, true));
        // A failed answer is never applied, but ends the wait.
        assert_eq!(queries.ask(later), Some(3));
        assert!(!queries.answered(3, false));
        assert_eq!(queries.ask(later), Some(4));
    }

    #[test]
    fn a_stale_answer_does_not_end_the_wait_for_the_current_question() {
        let now = Instant::now();
        let later = now + AVAILABILITY_TIMEOUT;
        let mut queries = AvailabilityQueries::default();
        queries.ask(now);
        queries.ask(later);
        queries.answered(1, true);
        assert_eq!(queries.ask(later), None);
    }
}
