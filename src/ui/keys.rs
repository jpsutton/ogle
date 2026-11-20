//! The hotkey table (01 section 5.2). One table drives both the dispatcher in `app` and
//! the help screen (UI-040), so they cannot drift apart. Reserved keys (UI-048) are
//! simply absent from the table.

use crate::modules::processes::SortKey;
use crate::modules::{DiskioMode, ModuleId, World};
use crate::term::Key;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Sort,
    Processes,
    Display,
    Modules,
    Alerts,
    Other,
}

/// Help screen group order (UI-110).
pub const GROUPS: [(Group, &str); 6] = [
    (Group::Sort, "Sort"),
    (Group::Processes, "Processes"),
    (Group::Display, "Display"),
    (Group::Modules, "Modules"),
    (Group::Alerts, "Alerts"),
    (Group::Other, "Other"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    FilterPopup,
    ClearFilter,
    Sort(SortKey),
    ReverseSort,
    SortPrev,
    SortNext,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    ScrollLeft,
    ScrollRight,
    ScrollLeft8,
    ScrollRight8,
    KillPopup,
    ReniceUp,
    ReniceDown,
    Pin,
    Programs,
    LongNames,
    Normalize,
    ResetMinMax,
    Processes,
    Percpu,
    Sidebar,
    Quicklook,
    FullQuicklook,
    Top,
    Sparkline,
    NetBytes,
    NetSum,
    NetCumulative,
    DiskIops,
    DiskLatency,
    FsFree,
    Module(ModuleId),
    AlertPanel,
    ClearWarnings,
    ClearAllFinished,
    Help,
    Refresh,
    Quit,
    Suspend,
}

pub struct Binding {
    pub keys: &'static [Key],
    /// Key text on the help screen; empty = folded into the previous line.
    pub help_key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub action: Action,
}

const fn b(keys: &'static [Key], help_key: &'static str, label: &'static str, group: Group, action: Action) -> Binding {
    Binding { keys, help_key, label, group, action }
}

use self::Group as G;
use crate::term::Key as K;

pub static TABLE: &[Binding] = &[
    b(&[K::Char('a')], "a", "Sort processes automatically", G::Sort, Action::Sort(SortKey::Auto)),
    b(&[K::Char('c')], "c", "Sort by CPU%", G::Sort, Action::Sort(SortKey::Cpu)),
    b(&[K::Char('m')], "m", "Sort by MEM%", G::Sort, Action::Sort(SortKey::Mem)),
    b(&[K::Char('i')], "i", "Sort by I/O rate", G::Sort, Action::Sort(SortKey::Io)),
    b(&[K::Char('t')], "t", "Sort by CPU time", G::Sort, Action::Sort(SortKey::Time)),
    b(&[K::Char('p')], "p", "Sort by name", G::Sort, Action::Sort(SortKey::Name)),
    b(&[K::Char('u')], "u", "Sort by user", G::Sort, Action::Sort(SortKey::User)),
    b(&[K::Char('o')], "o", "Sort by CPU core number", G::Sort, Action::Sort(SortKey::Core)),
    b(&[K::Char('P')], "P", "Sort by PID", G::Sort, Action::Sort(SortKey::Pid)),
    b(&[K::Char('r')], "r", "Reverse the sort direction", G::Sort, Action::ReverseSort),
    b(&[K::ShiftLeft], "Shift-Left/Right", "Previous / next sort key", G::Sort, Action::SortPrev),
    b(&[K::ShiftRight], "", "", G::Sort, Action::SortNext),
    b(&[K::Enter], "Enter", "Edit the process filter", G::Processes, Action::FilterPopup),
    b(&[K::Char('E')], "E", "Clear the process filter", G::Processes, Action::ClearFilter),
    b(&[K::Up], "Up/Down", "Move the selection", G::Processes, Action::Up),
    b(&[K::Down], "", "", G::Processes, Action::Down),
    b(&[K::PageUp], "PgUp/PgDn", "Move the selection by one page", G::Processes, Action::PageUp),
    b(&[K::PageDown], "", "", G::Processes, Action::PageDown),
    b(&[K::Home], "Home/End", "Select the first / last process", G::Processes, Action::Home),
    b(&[K::End], "", "", G::Processes, Action::End),
    b(&[K::Left], "Left/Right", "Scroll command arguments by 1", G::Processes, Action::ScrollLeft),
    b(&[K::Right], "", "", G::Processes, Action::ScrollRight),
    b(&[K::Char('<')], "</>", "Scroll command arguments by 8", G::Processes, Action::ScrollLeft8),
    b(&[K::Char('>')], "", "", G::Processes, Action::ScrollRight8),
    b(&[K::Char('k')], "k", "Kill the selected process", G::Processes, Action::KillPopup),
    b(&[K::Char('+')], "+", "Renice +1 (lower priority)", G::Processes, Action::ReniceUp),
    b(&[K::Char('-')], "-", "Renice -1 (higher priority)", G::Processes, Action::ReniceDown),
    b(&[K::Char('e')], "e", "Pin / unpin the selected process", G::Processes, Action::Pin),
    b(&[K::Char('j')], "j", "Program (grouped) view", G::Processes, Action::Programs),
    b(&[K::Char('/')], "/", "Long command names", G::Processes, Action::LongNames),
    b(&[K::Char('M')], "M", "Reset filter min/max rows", G::Processes, Action::ResetMinMax),
    b(&[K::Char('0')], "0", "Normalise CPU% and load per core", G::Display, Action::Normalize),
    b(&[K::Char('1')], "1", "Per-CPU mode", G::Display, Action::Percpu),
    b(&[K::Char('2')], "2", "Left sidebar", G::Display, Action::Sidebar),
    b(&[K::Char('3')], "3", "Quicklook", G::Display, Action::Quicklook),
    b(&[K::Char('4')], "4", "Full quicklook mode", G::Display, Action::FullQuicklook),
    b(&[K::Char('5')], "5", "Top row", G::Display, Action::Top),
    b(&[K::Char('S')], "S", "Quicklook sparklines", G::Display, Action::Sparkline),
    b(&[K::Char('b')], "b", "Network rates in bytes", G::Display, Action::NetBytes),
    b(&[K::Char('T')], "T", "Network combined Rx+Tx", G::Display, Action::NetSum),
    b(&[K::Char('U')], "U", "Network cumulative totals", G::Display, Action::NetCumulative),
    b(&[K::Char('B')], "B", "Disk I/O operations per second", G::Display, Action::DiskIops),
    b(&[K::Char('L')], "L", "Disk I/O latency", G::Display, Action::DiskLatency),
    b(&[K::Char('F')], "F", "File system free space", G::Display, Action::FsFree),
    b(&[K::Char('z')], "z", "Process collection", G::Modules, Action::Processes),
    b(&[K::Char('n')], "n", "Network module", G::Modules, Action::Module(ModuleId::Network)),
    b(&[K::Char('d')], "d", "Disk I/O module", G::Modules, Action::Module(ModuleId::Diskio)),
    b(&[K::Char('f')], "f", "File system module", G::Modules, Action::Module(ModuleId::Fs)),
    b(&[K::Char('s')], "s", "Sensors module", G::Modules, Action::Module(ModuleId::Sensors)),
    b(&[K::Char('N')], "N", "Clock module", G::Modules, Action::Module(ModuleId::Now)),
    b(&[K::Char('l')], "l", "Alert panel", G::Modules, Action::AlertPanel),
    b(&[K::Char('w')], "w", "Delete finished alerts (not CRITICAL)", G::Alerts, Action::ClearWarnings),
    b(&[K::Char('x')], "x", "Delete all finished alerts", G::Alerts, Action::ClearAllFinished),
    b(&[K::Char('h')], "h", "Help screen", G::Other, Action::Help),
    b(&[K::F(5), K::Ctrl('r')], "F5/Ctrl-R", "Refresh now and redraw", G::Other, Action::Refresh),
    b(&[K::Char('q'), K::Esc, K::Ctrl('c')], "q/Esc/Ctrl-C", "Quit", G::Other, Action::Quit),
    b(&[K::Suspend], "Ctrl-Z", "Suspend", G::Other, Action::Suspend),
];

/// Find the binding for a key. With `arrow_keys_sort`, plain and shifted Left/Right
/// are swapped before the lookup (UI-047).
pub fn lookup(key: Key, arrow_keys_sort: bool) -> Option<&'static Binding> {
    let key = if arrow_keys_sort {
        match key {
            Key::Left => Key::ShiftLeft,
            Key::Right => Key::ShiftRight,
            Key::ShiftLeft => Key::Left,
            Key::ShiftRight => Key::Right,
            k => k,
        }
    } else {
        key
    };
    TABLE.iter().find(|b| b.keys.contains(&key))
}

/// Current state of a toggle action, for the help screen (UI-110). None = not a toggle.
pub fn state(a: Action, w: &World) -> Option<bool> {
    let ui = &w.ui;
    Some(match a {
        Action::Programs => ui.programs,
        Action::LongNames => ui.long_names,
        Action::Normalize => ui.normalize_cpu,
        Action::Processes => ui.processes,
        Action::Percpu => ui.percpu,
        Action::Sidebar => ui.left_sidebar,
        Action::Quicklook => ui.quicklook,
        Action::FullQuicklook => ui.full_quicklook,
        Action::Top => ui.top,
        Action::Sparkline => ui.sparkline,
        Action::NetBytes => ui.net_bytes,
        Action::NetSum => ui.net_sum,
        Action::NetCumulative => ui.net_cumulative,
        Action::DiskIops => ui.diskio_mode == DiskioMode::Iops,
        Action::DiskLatency => ui.diskio_mode == DiskioMode::Latency,
        Action::FsFree => ui.fs_free,
        Action::Module(m) => ui.on(m),
        Action::AlertPanel => ui.alert_panel,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicate_keys_and_no_reserved() {
        let mut seen = Vec::new();
        for b in TABLE {
            for k in b.keys {
                assert!(!seen.contains(k), "{k:?} bound twice");
                seen.push(*k);
            }
        }
        for c in "678ACDGIKQRVWg".chars() {
            assert!(lookup(Key::Char(c), false).is_none());
        }
    }

    #[test]
    fn arrow_swap() {
        assert_eq!(lookup(Key::Left, true).unwrap().action, Action::SortPrev);
        assert_eq!(lookup(Key::ShiftRight, true).unwrap().action, Action::ScrollRight);
        assert_eq!(lookup(Key::Left, false).unwrap().action, Action::ScrollLeft);
    }
}
