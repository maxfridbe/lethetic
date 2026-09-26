use super::*;
use crate::icons;

impl App {
    pub fn sync_scroll_to_end(&mut self) {
        if self.total_line_count > 0 {
            self.output_state
                .select(Some(self.total_line_count.saturating_sub(1)));
        }
    }
    /// Palette commands filtered and ranked by the typed query.
    pub fn palette_matches(&self) -> Vec<CommandId> {
        crate::fuzzy::rank(
            &self.palette_query,
            &self.palette_items,
            |command| self.command_view(*command).label,
            |command| self.command_view(*command).description,
        )
    }

    pub fn next_palette_item(&mut self) {
        let count = self.palette_matches().len();
        if count == 0 {
            return;
        }
        let i = match self.palette_state.selected() {
            Some(i) => {
                if i >= count - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.palette_state.select(Some(i));
        self.should_redraw = true;
    }

    pub fn previous_palette_item(&mut self) {
        let count = self.palette_matches().len();
        if count == 0 {
            return;
        }
        let i = match self.palette_state.selected() {
            Some(i) => {
                if i == 0 {
                    count - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.palette_state.select(Some(i));
        self.should_redraw = true;
    }

    pub fn scroll_output_down(&mut self, amount: usize) {
        if self.total_line_count == 0 {
            return;
        }
        let current = self.output_state.selected().unwrap_or(0);
        let next = if current + amount >= self.total_line_count.saturating_sub(1) {
            self.total_line_count.saturating_sub(1)
        } else {
            current + amount
        };
        self.output_state.select(Some(next));
        self.auto_scroll = next >= self.total_line_count.saturating_sub(1);
        self.should_redraw = true;
    }

    pub fn scroll_output_up(&mut self, amount: usize) {
        if self.total_line_count == 0 {
            return;
        }
        let current = self.output_state.selected().unwrap_or(0);
        let next = current.saturating_sub(amount);
        self.output_state.select(Some(next));
        self.auto_scroll = false;
        self.should_redraw = true;
    }

    pub fn scroll_to_top(&mut self) {
        self.output_state.select(Some(0));
        self.auto_scroll = false;
        self.should_redraw = true;
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.total_line_count > 0 {
            self.output_state
                .select(Some(self.total_line_count.saturating_sub(1)));
            self.auto_scroll = true;
            self.should_redraw = true;
        }
    }
    pub fn tick_spinner(&mut self) {
        self.spinner_index = (self.spinner_index + 1) % icons::SPINNER.len();
        self.tool_spinner_index = (self.tool_spinner_index + 1) % icons::TOOL_SPINNER.len();
        self.should_redraw = true;
    }
}
