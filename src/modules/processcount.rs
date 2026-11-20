//! processcount panel (PROC-031..034).

use crate::modules::processes::{human_sort, SortKey};
use crate::modules::{Area, World};
use crate::ui::panel::{Align, Panel, Role};

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let _ = area;
    if !w.ui.processes {
        // PROC-029 / PROC-031.
        out.add(Role::Title, 0, "PROCESSES DISABLED (press 'z' to display)");
        return Align::Top;
    }
    let p = &w.processes;
    if let Some(f) = &p.filter {
        out.add(Role::Title, 0, "Processes filter:");
        crate::addf!(out, Role::Filter, 0, " {}", f.pattern());
        if let Some(k) = f.key_name() {
            crate::addf!(out, Role::Default, 0, " on column {}", k);
        }
        out.add(Role::Default, 0, " ('ENTER' to edit, 'E' to reset)");
        out.newline();
    }
    if !p.focus.is_empty() {
        out.add(Role::Title, 0, "Focus on:");
        for (k, f) in p.focus.iter().enumerate() {
            out.add(Role::Filter, 0, if k == 0 { " " } else { "," });
            out.add(Role::Filter, 0, &f.text);
        }
        out.newline();
    }
    // PROC-032: counts before filter and focus, after kernel-thread hiding.
    let c = p.counts;
    out.add(Role::Title, 0, "TASKS");
    crate::addf!(
        out,
        Role::Default,
        0,
        "{:>4} ({} thr), {} run, {} slp, {} oth ",
        c.total,
        c.thr,
        c.run,
        c.slp,
        c.total.saturating_sub(c.run + c.slp)
    );
    out.add(Role::Default, 0, if w.ui.programs { "Programs" } else { "Threads" });
    if p.sort == SortKey::Auto {
        crate::addf!(out, Role::Default, 0, " sorted automatically by {}", human_sort(p.eff));
    } else {
        crate::addf!(out, Role::Default, 0, " sorted by {}", human_sort(p.eff));
    }
    Align::Top
}
