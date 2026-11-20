//! Help screen (UI-110 to UI-113), generated from the key table in `ui::keys`.

use crate::modules::World;
use crate::term::Screen;
use crate::ui::keys::{self, Group, GROUPS, TABLE};
use crate::ui::layout::blit;
use crate::ui::panel::{Panel, Role};

const LEGEND: [(Role, &str); 8] = [
    (Role::Ok, "OK"),
    (Role::Careful, "CAREFUL"),
    (Role::Warning, "WARNING"),
    (Role::Critical, "CRITICAL"),
    (Role::OkLog, "OK_LOG"),
    (Role::CarefulLog, "CAREFUL_LOG"),
    (Role::WarningLog, "WARNING_LOG"),
    (Role::CriticalLog, "CRITICAL_LOG"),
];

/// Row where the key table starts (after version, config path and a blank line).
const KEYS_ROW: u16 = 3;

fn group_lines(g: Group) -> usize {
    1 + TABLE.iter().filter(|b| b.group == g && !b.help_key.is_empty()).count()
}

/// Append one group (title + bindings + blank line). Returns lines added.
fn add_group(p: &mut Panel, w: &World, g: Group, name: &str, kw: usize) -> usize {
    p.add(Role::Title, 0, name);
    p.newline();
    for b in TABLE.iter().filter(|b| b.group == g && !b.help_key.is_empty()) {
        p.addf(Role::Default, 0, format_args!("{:>kw$}  {}", b.help_key, b.label));
        if let Some(on) = keys::state(b.action, w) {
            p.add(Role::Default, 0, if on { " [on]" } else { " [off]" });
        }
        p.newline();
    }
    p.newline();
    group_lines(g) + 1
}

/// Draw the help screen. `scroll` is clamped to the content height here.
pub fn draw(w: &World, scr: &mut Screen, left: &mut Panel, right: &mut Panel, scroll: &mut u16) {
    let (sw, sh) = scr.size();
    let two = sw >= 100;
    let kw = TABLE.iter().map(|b| b.help_key.len()).max().unwrap_or(0);
    left.clear();
    right.clear();
    left.addf(Role::Title, 0, format_args!("ogle {}", env!("CARGO_PKG_VERSION")));
    left.newline();
    match &w.cfg.source {
        Some(p) => left.addf(Role::Default, 0, format_args!("Configuration file: {}", p.display())),
        None => left.add(Role::Default, 0, "Configuration file: none"),
    }
    left.newline();
    left.newline();

    // Split the groups into two columns of about equal height.
    let total: usize = GROUPS.iter().map(|&(g, _)| group_lines(g) + 1).sum();
    let mut acc = 0;
    let (mut ln, mut rn) = (0, 0);
    for &(g, name) in GROUPS.iter() {
        if two && acc >= total / 2 {
            rn += add_group(right, w, g, name, kw);
        } else {
            let n = add_group(left, w, g, name, kw);
            acc += n;
            ln += n;
        }
    }
    while ln < rn {
        left.newline();
        ln += 1;
    }
    for (role, name) in LEGEND {
        left.add(role, 0, name);
        left.newline();
    }

    let body_h = sh.saturating_sub(1);
    let max_scroll = (left.n_lines() as u16).saturating_sub(body_h);
    *scroll = (*scroll).min(max_scroll);
    let s = *scroll;
    let col_w = if two { sw / 2 } else { sw };
    blit(scr, left, s as usize, 0, 0, col_w, body_h, 0, true);
    if two {
        let (first, y) = if s <= KEYS_ROW { (0, KEYS_ROW - s) } else { ((s - KEYS_ROW) as usize, 0) };
        if y < body_h {
            blit(scr, right, first, col_w, y, sw - col_w, body_h - y, 0, true);
        }
    }
    if sh > 0 {
        scr.put(0, sh - 1, sw, "h/q/Esc: close   Up/Down/PgUp/PgDn: scroll", Role::Default);
    }
}
