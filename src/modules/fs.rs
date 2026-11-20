//! File system module (03-io-modules.md section 4).
//!
//! Usage queries for local physical mounts call `statvfs` (or the platform equivalent)
//! inline, which is cheap. Mounts that may block (non-physical or network types added
//! through `allow`) are queried on one lazily spawned helper thread (PLT-010, IO-058,
//! PERF-030) with a shared 2 s deadline per fs cycle.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::{auto_unit, MinSym};
use crate::modules::network::{cell, cfg_refresh, header, name_width, register_item, thresholds_cfg, Aliases, Filter, Fit, Trunc};
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{FsUsage, Mount, Platform, Sys, FS_TIMEOUT};
use crate::ui::panel::{str_width, Align, Panel, Role};

const MODULE: &str = "fs";
/// IO-064 compiled-in default.
const FS_DEFAULT: Limits = Limits::new(50.0, 70.0, 90.0, false);
/// IO-023 default refresh.
const DEFAULT_REFRESH: Duration = Duration::from_secs(60);

struct FsItem {
    mp: Arc<str>,
    device: String,
    alias: Option<String>,
    /// Passed hide/show on mount point, device or alias (IO-057).
    allowed: bool,
    seen: bool,
    read_only: bool,
    /// Query may block: goes through the helper thread.
    remote: bool,
    /// Identity for helper results (survives reordering, never reused).
    key: u64,
    /// A helper request is in flight (at most one per mount).
    pending: bool,
    /// Request sent during the current cycle.
    sent_now: bool,
    /// None = unknown (timed out or failed): cells show `?` (IO-058).
    usage: Option<FsUsage>,
    id: Option<StatId>,
    role: Role,
}

impl FsItem {
    fn disp(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.mp)
    }
    /// IO-059: mounts with total = 0 are skipped.
    fn shown(&self) -> bool {
        self.allowed && !matches!(self.usage, Some(u) if u.total == 0)
    }
}

/// Helper thread endpoints. The thread exits when `tx` is dropped (unless it is stuck in
/// a hung query, in which case it dies with the process).
struct Worker {
    tx: Sender<(u64, Arc<str>)>,
    rx: Receiver<(u64, Option<FsUsage>)>,
}

impl Worker {
    fn spawn() -> Option<Worker> {
        let (tx, wrx) = channel::<(u64, Arc<str>)>();
        let (wtx, rx) = channel::<(u64, Option<FsUsage>)>();
        std::thread::Builder::new()
            .name("ogle-fs".to_string())
            .spawn(move || {
                while let Ok((key, mp)) = wrx.recv() {
                    let u = <Sys as Platform>::fs_usage(&mp);
                    if wtx.send((key, u)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Worker { tx, rx })
    }
}

pub struct Fs {
    filter: Filter,
    aliases: Aliases,
    tcfg: Config,
    /// IO-055 allow list (substring match on fs_type).
    allow: Vec<String>,
    refresh: Duration,
    /// Sorted by real mount point, plain code-point order (IO-060).
    items: Vec<FsItem>,
    mounts: Vec<Mount>,
    worker: Option<Worker>,
    spawn_failed: bool,
    next_key: u64,
}

impl Fs {
    pub fn new(cfg: &Config, _ui: &UiState, _alerts: &mut Alerts) -> Self {
        Fs {
            filter: Filter::new(cfg, MODULE, &["/boot.*", ".*/snap.*"]), // IO-057
            aliases: Aliases::new(cfg, MODULE),
            tcfg: thresholds_cfg(cfg, MODULE),
            allow: cfg.str_list(&[MODULE, "allow"]).unwrap_or_default(),
            refresh: cfg_refresh(cfg, MODULE).unwrap_or(DEFAULT_REFRESH),
            items: Vec::new(),
            mounts: Vec::new(),
            worker: None,
            spawn_failed: false,
            next_key: 0,
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        Some(self.refresh)
    }

    /// IO-054 / IO-055.
    fn type_ok(&self, m: &Mount) -> bool {
        m.is_physical || self.allow.iter().any(|a| m.fs_type.contains(a.as_str()))
    }

    pub fn update(&mut self, sys: &mut Sys, _ctx: &Ctx, _ui: &UiState, alerts: &mut Alerts) {
        sys.filesystems(&mut self.mounts);
        for it in &mut self.items {
            it.seen = false;
        }
        // Later entries overwrite earlier ones, so a stacked mount point keeps the
        // entry mounted last (IO-056).
        for m in &self.mounts {
            if !self.type_ok(m) {
                continue;
            }
            let remote = !m.is_physical || m.is_network;
            match self.items.binary_search_by(|i| (*i.mp).cmp(m.mount_point.as_str())) {
                Ok(i) => {
                    let it = &mut self.items[i];
                    if it.device != m.device {
                        it.device.clear();
                        it.device.push_str(&m.device);
                        let keys = [&*it.mp, it.device.as_str(), it.alias.as_deref().unwrap_or("")];
                        let allowed = self.filter.allows(&keys);
                        it.allowed = allowed;
                    }
                    it.read_only = m.read_only;
                    it.remote = remote;
                    it.seen = true;
                }
                Err(pos) => {
                    let alias = self.aliases.get(&m.mount_point).map(str::to_string);
                    let allowed =
                        self.filter.allows(&[&m.mount_point, &m.device, alias.as_deref().unwrap_or("")]);
                    let id = if allowed {
                        register_item(&self.tcfg, alerts, MODULE, &m.mount_point, alias.as_deref(), None, Some(FS_DEFAULT))
                    } else {
                        None
                    };
                    self.next_key += 1;
                    let item = FsItem {
                        mp: Arc::from(m.mount_point.as_str()),
                        device: m.device.clone(),
                        alias,
                        allowed,
                        seen: true,
                        read_only: m.read_only,
                        remote,
                        key: self.next_key,
                        pending: false,
                        sent_now: false,
                        usage: None,
                        id,
                        role: Role::Default,
                    };
                    self.items.insert(pos, item);
                }
            }
        }
        self.items.retain(|it| {
            if !it.seen {
                if let Some(id) = it.id {
                    alerts.unregister(id);
                }
            }
            it.seen
        });

        // Late results from earlier cycles: the mount is free for a new query now.
        if let Some(w) = &self.worker {
            while let Ok((key, u)) = w.rx.try_recv() {
                apply(&mut self.items, key, u);
            }
        }

        // Query usage. IO-014: hidden mounts cost no query.
        let mut outstanding = 0usize;
        for it in self.items.iter_mut() {
            it.sent_now = false;
            if !it.allowed {
                continue;
            }
            if !it.remote {
                it.usage = <Sys as Platform>::fs_usage(&it.mp);
                continue;
            }
            if it.pending {
                // Still stuck from an earlier cycle: not retried (PLT-010).
                it.usage = None;
                continue;
            }
            if self.worker.is_none() && !self.spawn_failed {
                self.worker = Worker::spawn();
                self.spawn_failed = self.worker.is_none();
            }
            match &self.worker {
                Some(w) if w.tx.send((it.key, it.mp.clone())).is_ok() => {
                    it.pending = true;
                    it.sent_now = true;
                    outstanding += 1;
                }
                _ => it.usage = None,
            }
        }

        // Wait for this cycle's answers, never longer than FS_TIMEOUT in total.
        let mut dead = false;
        if outstanding > 0 {
            if let Some(w) = &self.worker {
                let deadline = Instant::now() + FS_TIMEOUT;
                while outstanding > 0 {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match w.rx.recv_timeout(deadline - now) {
                        Ok((key, u)) => {
                            if apply(&mut self.items, key, u) {
                                outstanding -= 1;
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            dead = true;
                            break;
                        }
                    }
                }
            }
        }
        if dead {
            // The helper died (it should not); a new one is spawned on the next cycle.
            self.worker = None;
        }
        for it in &mut self.items {
            if it.pending && (it.sent_now || dead) {
                it.usage = None; // IO-058: timed out, show `?`, retry later.
            }
            if dead {
                it.pending = false;
            }
        }

        // IO-064..IO-066.
        for it in &mut self.items {
            it.role = Role::Default;
            if !it.allowed || it.read_only {
                continue;
            }
            if let (Some(id), Some(u)) = (it.id, it.usage) {
                if u.total > 0 {
                    it.role = alerts.eval(id, u.total.saturating_sub(u.avail) as f64, u.total as f64);
                }
            }
        }
    }
}

/// Store a helper result. Returns true when the request was sent this cycle.
fn apply(items: &mut [FsItem], key: u64, u: Option<FsUsage>) -> bool {
    match items.iter_mut().find(|i| i.key == key) {
        Some(it) => {
            it.pending = false;
            it.usage = u;
            it.sent_now
        }
        None => false,
    }
}

/// `os.path.basename` equivalent for the device column (IO-061).
fn basename(s: &str) -> &str {
    s.rsplit(|c: char| c == '/' || c == '\\').next().unwrap_or("")
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let fs = &w.fs;
    let free = w.ui.fs_free;
    let Some(n) = name_width(area) else { return Align::Top };
    if !fs.items.iter().any(FsItem::shown) {
        return Align::Top; // IO-005
    }
    header(out, "FILE SYS", n, &[if free { "Free" } else { "Used" }, "Total"], 7);
    let mut rows = 1u16;
    for it in fs.items.iter().filter(|it| it.shown()) {
        if rows >= area.max_height {
            break; // IO-006
        }
        rows += 1;
        out.newline();
        // IO-061 label.
        let label = it.disp();
        let base = basename(&it.device);
        let (lw, bw) = (str_width(label), str_width(base));
        if !base.is_empty() && lw + bw <= n.saturating_sub(4) {
            let pad = n - (lw + bw + 3);
            out.addf(Role::Default, 0, format_args!("{} ({}){:pad$}", label, base, "", pad = pad));
        } else {
            out.addf(Role::Default, 0, format_args!("{}", Fit { s: label, width: n, mode: Trunc::Head }));
        }
        // IO-062 cells.
        match it.usage {
            Some(u) => {
                let v = if free { u.avail } else { u.total.saturating_sub(u.free) };
                cell(out, it.role, auto_unit(v, false, MinSym::None).as_str(), 7);
                cell(out, Role::Default, auto_unit(u.total, false, MinSym::None).as_str(), 7);
            }
            None => {
                cell(out, Role::Default, "?", 7);
                cell(out, Role::Default, "?", 7);
            }
        }
    }
    Align::Top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_basename() {
        assert_eq!(basename("/dev/nvme0n1p3"), "nvme0n1p3");
        assert_eq!(basename("tmpfs"), "tmpfs");
        assert_eq!(basename("C:\\"), "");
    }
}
