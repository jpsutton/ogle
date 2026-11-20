//! Status message area on the last screen row (UI-114).

use std::time::{Duration, Instant};

use crate::term::Screen;
use crate::ui::panel::Role;

const SHOW_FOR: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Status {
    text: String,
    error: bool,
    until: Option<Instant>,
}

impl Status {
    /// Show `msg` for 3 s, replacing any current message. Empty messages are ignored.
    pub fn set(&mut self, msg: &str, error: bool, now: Instant) {
        if msg.is_empty() {
            return;
        }
        self.text.clear();
        self.text.push_str(msg);
        self.error = error;
        self.until = Some(now + SHOW_FOR);
    }

    /// Show an action result; failures (text starting with "Cannot" or "Refusing") use
    /// the ERROR role, everything else INFO.
    pub fn set_result(&mut self, msg: &str, now: Instant) {
        let err = msg.starts_with("Cannot") || msg.starts_with("Refusing");
        self.set(msg, err, now);
    }

    /// Expiry time of the current message, for the main-loop wait.
    pub fn until(&self) -> Option<Instant> {
        self.until
    }

    /// Drop the message once it expired. Returns true when something was dropped.
    pub fn expire(&mut self, now: Instant) -> bool {
        match self.until {
            Some(t) if now >= t => {
                self.until = None;
                true
            }
            _ => false,
        }
    }

    pub fn draw(&self, scr: &mut Screen) {
        if self.until.is_none() {
            return;
        }
        let (w, h) = scr.size();
        if h == 0 {
            return;
        }
        let role = if self.error { Role::Error } else { Role::Info };
        scr.fill(0, h - 1, w, ' ', Role::Default);
        scr.put(0, h - 1, w, &self.text, role);
    }
}
