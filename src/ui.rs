mod block;
mod chat;
mod layout;
mod model_catalog;
mod overlays;
mod python_setup;
mod remote_control;
mod status;

pub use crate::theme::Theme;
pub use block::{render_block_to_lines, render_block_to_lines_with_cost_visibility};
pub use layout::centered_rect;

use crate::app::App;

pub use chat::block_header_at_line;

pub fn ui(f: &mut ratatui::Frame, app: &mut App) {
    layout::render_background(f, app);
    let areas = layout::calculate_areas(app, f.area());
    chat::render_output(f, app, areas.output);
    chat::render_input(f, app, areas.input, areas.inner_width);
    status::render_processing(f, app, areas.processing);
    status::render_status(f, app, areas.status);
    status::render_debug(f, app, areas.debug);
    if overlays::render_primary(f, app) {
        model_catalog::render(f, app);
        remote_control::render(f, app);
        crate::compact::render_compaction_popup(f, app);
        return;
    }
    if app.python_setup.is_some() {
        python_setup::render(f, app);
    }
    overlays::render_secondary(f, app);
    model_catalog::render(f, app);
    remote_control::render(f, app);
    crate::compact::render_compaction_popup(f, app);
}
