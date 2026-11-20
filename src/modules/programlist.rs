//! programlist panel (programs mode, PROC-130..134). Shares the column code of
//! `processlist`.

use crate::modules::{Area, World};
use crate::ui::panel::{Align, Panel};

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    crate::modules::processlist::render_list(w, area, out, true)
}
