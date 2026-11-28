//! Asks logind, over the system bus, whether the machine can suspend,
//! restart and power off, and to do it.
//!
//! The calls go through `busctl` on a thread of their own, the way rustwm
//! reaches every other outside service, so a slow answer, such as one
//! waiting on a polkit password prompt, never holds up drawing. The answer
//! comes back to the event loop through a channel. The calls are rare, so
//! starting a process for each costs nothing worth a D-Bus library and an
//! async runtime, and busctl brings a timeout and logind's error text.

use std::{
    process::{Command, Output, Stdio},
    thread,
};

use serde::Deserialize;
use smithay::reexports::calloop::{
    LoopHandle,
    channel::{self, Sender},
};
use tracing::warn;

use crate::state::State;

/// How long a question such as `CanSuspend` may take, in seconds.
const QUERY_TIMEOUT: u32 = 5;
/// How long logind may take to act, in seconds: long enough for a polkit
/// agent to ask for a password.
const PERFORM_TIMEOUT: u32 = 120;

/// What logind can do to the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Suspend,
    Reboot,
    PowerOff,
}

impl PowerAction {
    /// The logind method that does it.
    fn method(self) -> &'static str {
        match self {
            PowerAction::Suspend => "Suspend",
            PowerAction::Reboot => "Reboot",
            PowerAction::PowerOff => "PowerOff",
        }
    }

    /// What doing it is called in a message, such as "Restarting is not
    /// allowed right now."
    fn doing(self) -> &'static str {
        match self {
            PowerAction::Suspend => "Suspending",
            PowerAction::Reboot => "Restarting",
            PowerAction::PowerOff => "Powering off",
        }
    }

    /// The logind method that says whether it can be done.
    fn can_method(self) -> &'static str {
        match self {
            PowerAction::Suspend => "CanSuspend",
            PowerAction::Reboot => "CanReboot",
            PowerAction::PowerOff => "CanPowerOff",
        }
    }
}

/// Which [`PowerAction`]s logind offers. The default, before
/// logind has answered, offers none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Availability {
    pub suspend: bool,
    pub reboot: bool,
    pub power_off: bool,
}

impl Availability {
    pub fn allows(self, power: PowerAction) -> bool {
        match power {
            PowerAction::Suspend => self.suspend,
            PowerAction::Reboot => self.reboot,
            PowerAction::PowerOff => self.power_off,
        }
    }
}

/// logind's answer to `CanSuspend` and the like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Can {
    Yes,
    /// Allowed only after authentication through a polkit agent. rustwm
    /// cannot ask for it, so it counts as unavailable.
    Challenge,
    /// Not allowed for this user.
    No,
    /// Not supported by the machine.
    Na,
}

impl Can {
    /// Whether rustwm can go ahead: only on a plain "yes", since an action
    /// that needs authentication would always be refused (see
    /// [`check_answer`]), so the search does not offer it.
    fn is_available(self) -> bool {
        self == Can::Yes
    }
}

/// A program holding off a shutdown or sleep, from `ListInhibitors`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inhibitor {
    /// What it holds off, such as "shutdown" or "shutdown:sleep".
    pub what: String,
    /// The program, such as "Software Updates".
    pub who: String,
    /// Why, such as "Installing updates".
    pub why: String,
    /// "block", or "delay" for one that only holds off for a moment.
    pub mode: String,
}

/// A question or command for logind. Each carries the ticket of the dialog
/// that asked, so a reply for a dialog that has since been cancelled can be
/// dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Which actions logind offers; `seq` numbers the questions, so a late
    /// answer cannot replace a newer one.
    Availability {
        seq: u64,
    },
    /// Which programs block a shutdown.
    Inhibitors {
        ticket: u64,
    },
    /// Whether `power` is allowed right now, asked just before windows are
    /// closed for it.
    Check {
        ticket: u64,
        power: PowerAction,
    },
    Perform {
        ticket: u64,
        power: PowerAction,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The answer to availability question `seq`; `None` when busctl
    /// failed, so the last answer still holds.
    Availability {
        seq: u64,
        availability: Option<Availability>,
    },
    /// The programs blocking a shutdown; empty when busctl failed, since
    /// the dialog then simply shows none.
    Inhibitors {
        ticket: u64,
        inhibitors: Vec<Inhibitor>,
    },
    /// Whether `power` is allowed, or why not.
    Checked {
        ticket: u64,
        power: PowerAction,
        result: Result<(), String>,
    },
    /// logind accepted the command, or did not.
    Done {
        ticket: u64,
        power: PowerAction,
        result: Result<(), Failure>,
    },
}

/// Why a command did not go through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// logind, or busctl, said no, with this message.
    Refused(String),
    /// No answer came in time. logind may still be carrying the command
    /// out, so this must not be shown as a failure.
    NoAnswer,
}

impl Failure {
    /// What the dialog says.
    pub fn message(&self) -> String {
        match self {
            Failure::Refused(message) => message.clone(),
            Failure::NoAnswer => {
                "logind did not answer in time; the system may still be shutting down.".to_string()
            }
        }
    }
}

/// The failure busctl's `message` stands for: no answer when it timed out.
fn failure_of(message: String) -> Failure {
    let lower = message.to_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        Failure::NoAnswer
    } else {
        Failure::Refused(message)
    }
}

/// Refusal for every request in a nested session.
const NESTED: &str = "Not available in a nested session.";

pub struct Logind {
    sender: Sender<Reply>,
    /// Nested, logind is the host's: it is never asked anything, whatever
    /// path a request comes from, so a nested rustwm cannot suspend,
    /// restart or power off the machine it runs on.
    nested: bool,
}

impl Logind {
    pub fn new(loop_handle: &LoopHandle<'static, State>, nested: bool) -> anyhow::Result<Self> {
        let (sender, receiver) = channel::channel();
        loop_handle
            .insert_source(receiver, |event, _, state| {
                if let channel::Event::Msg(reply) = event {
                    state.logind_reply(reply);
                }
            })
            .map_err(|err| anyhow::anyhow!("failed to listen for logind replies: {err}"))?;
        Ok(Self { sender, nested })
    }

    /// Sends `request` to logind on a thread of its own; the reply comes
    /// back through [`State::logind_reply`].
    pub fn request(&self, request: Request) {
        if let Some(refusal) = refusal(request, self.nested) {
            warn!(?request, "logind is not asked in a nested session");
            let _ = self.sender.send(refusal);
            return;
        }
        let sender = self.sender.clone();
        let spawned = thread::Builder::new().name("logind".into()).spawn(move || {
            // Fails only when the compositor is shutting down.
            let _ = sender.send(answer(request));
        });
        if let Err(err) = spawned {
            warn!(%err, "failed to start a thread to ask logind");
            // The caller still hears back, so a dialog does not wait forever.
            let _ = self
                .sender
                .send(failure(request, format!("Could not ask logind: {err}.")));
        }
    }
}

/// The reply to `request` without asking logind, in a nested session;
/// `None` when logind may be asked.
fn refusal(request: Request, nested: bool) -> Option<Reply> {
    nested.then(|| failure(request, NESTED.to_string()))
}

/// Asks logind for `request`. Blocks while busctl runs.
fn answer(request: Request) -> Reply {
    match request {
        Request::Availability { seq } => Reply::Availability {
            seq,
            availability: availability(),
        },
        Request::Inhibitors { ticket } => Reply::Inhibitors {
            ticket,
            inhibitors: inhibitors(),
        },
        Request::Check { ticket, power } => Reply::Checked {
            ticket,
            power,
            result: check(power),
        },
        Request::Perform { ticket, power } => Reply::Done {
            ticket,
            power,
            // `true`: logind may let polkit ask for a password.
            result: busctl(power.method(), &["b", "true"], PERFORM_TIMEOUT)
                .map(|_| ())
                .map_err(failure_of),
        },
    }
}

/// The reply to `request` when it could not be asked at all.
fn failure(request: Request, message: String) -> Reply {
    match request {
        Request::Availability { seq } => Reply::Availability {
            seq,
            availability: None,
        },
        Request::Inhibitors { ticket } => Reply::Inhibitors {
            ticket,
            inhibitors: Vec::new(),
        },
        Request::Check { ticket, power } => Reply::Checked {
            ticket,
            power,
            result: Err(message),
        },
        Request::Perform { ticket, power } => Reply::Done {
            ticket,
            power,
            result: Err(Failure::Refused(message)),
        },
    }
}

/// Which actions logind offers, or `None` if any question failed.
fn availability() -> Option<Availability> {
    let can = |power: PowerAction| -> Option<bool> {
        match busctl(power.can_method(), &[], QUERY_TIMEOUT) {
            Ok(json) => Some(parse_can(&json)?.is_available()),
            Err(message) => {
                warn!(method = power.can_method(), message, "cannot ask logind");
                None
            }
        }
    };
    Some(Availability {
        suspend: can(PowerAction::Suspend)?,
        reboot: can(PowerAction::Reboot)?,
        power_off: can(PowerAction::PowerOff)?,
    })
}

/// Whether logind allows `power` right now, or a message saying why not.
fn check(power: PowerAction) -> Result<(), String> {
    let json = busctl(power.can_method(), &[], QUERY_TIMEOUT)?;
    check_answer(power, parse_can(&json))
}

/// What logind's `can` answer means for going ahead with `power`. A
/// "challenge", which can appear after the search showed the action, is
/// refused here, before any window is closed: logind would need
/// authentication that rustwm cannot ask for, so the request would only
/// fail once the windows were gone.
fn check_answer(power: PowerAction, can: Option<Can>) -> Result<(), String> {
    match can {
        Some(Can::Yes) => Ok(()),
        Some(Can::Challenge) => Err(format!(
            "{} needs authentication, which rustwm cannot ask for.",
            power.doing()
        )),
        Some(Can::No) => Err(format!("{} is not allowed right now.", power.doing())),
        Some(Can::Na) => Err(format!(
            "{} is not supported on this system.",
            power.doing()
        )),
        None => Err("logind gave an answer rustwm does not understand.".to_string()),
    }
}

/// The programs that block a shutdown, for the dialog to list.
fn inhibitors() -> Vec<Inhibitor> {
    let listed = busctl("ListInhibitors", &[], QUERY_TIMEOUT)
        .map_err(|message| warn!(message, "cannot list inhibitors"))
        .ok()
        .and_then(|json| parse_inhibitors(&json));
    blocking_shutdown(listed.unwrap_or_default())
}

/// Calls logind's manager `method` with `args` (a D-Bus signature followed
/// by the values) and returns busctl's JSON output, or logind's error.
fn busctl(method: &str, args: &[&str], timeout_secs: u32) -> Result<String, String> {
    let mut command = Command::new("busctl");
    command
        .args(["--system", "--json=short"])
        .arg(format!("--timeout={timeout_secs}"))
        .args([
            "call",
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            method,
        ])
        .args(args)
        .stdin(Stdio::null());
    let output = run(command).map_err(|err| format!("Could not run busctl: {err}."))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(error_message(&String::from_utf8_lossy(&output.stderr)))
    }
}

/// Runs busctl and waits for it.
#[cfg(not(test))]
fn run(mut command: Command) -> std::io::Result<Output> {
    command.output()
}

/// Tests never reach the real logind: one that did could suspend or power
/// off the machine running them.
#[cfg(test)]
fn run(_command: Command) -> std::io::Result<Output> {
    Err(std::io::Error::other("tests never run busctl"))
}

/// The shape of busctl's JSON output: the reply's values are in `data`.
#[derive(Deserialize)]
struct BusReply<T> {
    data: T,
}

/// Parses busctl's output for `CanSuspend` and the like, such as
/// `{"type":"s","data":["yes"]}`.
fn parse_can(json: &str) -> Option<Can> {
    let reply: BusReply<(String,)> = serde_json::from_str(json).ok()?;
    match reply.data.0.as_str() {
        "yes" => Some(Can::Yes),
        "challenge" => Some(Can::Challenge),
        "no" => Some(Can::No),
        "na" => Some(Can::Na),
        _ => None,
    }
}

/// Parses busctl's output for `ListInhibitors`, whose entries are what,
/// who, why, mode, user id and process id.
fn parse_inhibitors(json: &str) -> Option<Vec<Inhibitor>> {
    type Entry = (String, String, String, String, u32, u32);
    let reply: BusReply<(Vec<Entry>,)> = serde_json::from_str(json).ok()?;
    let inhibitors = reply
        .data
        .0
        .into_iter()
        .map(|(what, who, why, mode, _uid, _pid)| Inhibitor {
            what,
            who,
            why,
            mode,
        })
        .collect();
    Some(inhibitors)
}

/// The inhibitors that stop a shutdown outright. Delay inhibitors, such as
/// a browser saving its state, only hold it off for a moment, so they are
/// not worth listing, and neither is one that only holds off sleep.
pub fn blocking_shutdown(all: Vec<Inhibitor>) -> Vec<Inhibitor> {
    all.into_iter()
        .filter(|inhibitor| {
            inhibitor.mode == "block" && inhibitor.what.split(':').any(|what| what == "shutdown")
        })
        .collect()
}

/// logind's error from busctl's standard error, such as "Access denied"
/// from "Call failed: Access denied".
fn error_message(stderr: &str) -> String {
    let message = stderr.trim();
    let message = message.strip_prefix("Call failed: ").unwrap_or(message);
    if message.is_empty() {
        "logind did not answer.".to_string()
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_answers_parse() {
        let can = |answer: &str| parse_can(&format!(r#"{{"type":"s","data":["{answer}"]}}"#));
        assert_eq!(can("yes"), Some(Can::Yes));
        assert_eq!(can("challenge"), Some(Can::Challenge));
        assert_eq!(can("no"), Some(Can::No));
        assert_eq!(can("na"), Some(Can::Na));
        assert_eq!(can("maybe"), None);
        assert_eq!(parse_can("Call failed"), None);
        assert_eq!(parse_can(r#"{"type":"b","data":[true]}"#), None);
        assert!(Can::Yes.is_available());
        assert!(!Can::Challenge.is_available());
        assert!(!Can::Na.is_available());
    }

    fn inhibitor(what: &str, who: &str, mode: &str) -> Inhibitor {
        Inhibitor {
            what: what.into(),
            who: who.into(),
            why: "because".into(),
            mode: mode.into(),
        }
    }

    #[test]
    fn inhibitors_parse() {
        let json = r#"{"type":"a(ssssuu)","data":[[["sleep","UPower","Pause device polling","delay",0,2065],["shutdown","Google Chrome","Saving browser state","delay",1000,9824]]]}"#;
        let parsed = parse_inhibitors(json).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[1],
            Inhibitor {
                what: "shutdown".into(),
                who: "Google Chrome".into(),
                why: "Saving browser state".into(),
                mode: "delay".into(),
            }
        );
        assert_eq!(
            parse_inhibitors(r#"{"type":"a(ssssuu)","data":[[]]}"#),
            Some(Vec::new())
        );
        assert_eq!(parse_inhibitors("junk"), None);
    }

    #[test]
    fn only_blocking_shutdown_inhibitors_are_kept() {
        let all = vec![
            inhibitor("shutdown:sleep", "Updates", "block"),
            inhibitor("shutdown", "Google Chrome", "delay"),
            inhibitor("sleep", "Player", "block"),
            inhibitor("idle:shutdown", "Backup", "block"),
        ];
        let kept: Vec<String> = blocking_shutdown(all).into_iter().map(|i| i.who).collect();
        assert_eq!(kept, ["Updates", "Backup"]);
    }

    #[test]
    fn nested_logind_is_never_asked() {
        let refused = |request| refusal(request, true).expect("refused");
        assert_eq!(
            refused(Request::Availability { seq: 2 }),
            Reply::Availability {
                seq: 2,
                availability: None
            }
        );
        assert_eq!(
            refused(Request::Perform {
                ticket: 3,
                power: PowerAction::PowerOff
            }),
            Reply::Done {
                ticket: 3,
                power: PowerAction::PowerOff,
                result: Err(Failure::Refused(NESTED.to_string()))
            }
        );
        assert_eq!(
            refused(Request::Check {
                ticket: 4,
                power: PowerAction::Reboot
            }),
            Reply::Checked {
                ticket: 4,
                power: PowerAction::Reboot,
                result: Err(NESTED.to_string())
            }
        );
        assert_eq!(refusal(Request::Availability { seq: 1 }, false), None);
    }

    #[test]
    fn only_a_plain_yes_goes_ahead() {
        let reboot = PowerAction::Reboot;
        assert_eq!(check_answer(reboot, Some(Can::Yes)), Ok(()));
        assert_eq!(
            check_answer(PowerAction::PowerOff, Some(Can::Challenge)),
            Err("Powering off needs authentication, which rustwm cannot ask for.".to_string())
        );
        assert_eq!(
            check_answer(reboot, Some(Can::No)),
            Err("Restarting is not allowed right now.".to_string())
        );
        assert_eq!(
            check_answer(reboot, Some(Can::Na)),
            Err("Restarting is not supported on this system.".to_string())
        );
        assert!(check_answer(reboot, None).is_err());
    }

    #[test]
    fn a_timeout_does_not_claim_failure() {
        assert_eq!(failure_of("Connection timed out".into()), Failure::NoAnswer);
        assert_eq!(
            failure_of("Access denied".into()),
            Failure::Refused("Access denied".into())
        );
        assert!(Failure::NoAnswer.message().contains("may still be"));
    }

    #[test]
    fn tests_never_run_busctl() {
        assert!(busctl("PowerOff", &["b", "true"], 1).is_err());
    }

    #[test]
    fn error_message_is_logind_s_text() {
        assert_eq!(
            error_message("Call failed: Access denied\n"),
            "Access denied"
        );
        assert_eq!(
            error_message("Failed to connect to bus"),
            "Failed to connect to bus"
        );
        assert_eq!(error_message("  \n"), "logind did not answer.");
    }

    #[test]
    fn availability_allows_each_power() {
        let only_reboot = Availability {
            reboot: true,
            ..Availability::default()
        };
        assert!(only_reboot.allows(PowerAction::Reboot));
        assert!(!only_reboot.allows(PowerAction::Suspend));
        assert!(!only_reboot.allows(PowerAction::PowerOff));
    }
}
