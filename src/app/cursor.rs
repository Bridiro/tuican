//! Cursor and scroll movement, shared by every pane.

use super::action::Pane;
use super::{App, Scroll};

impl App {
    pub(super) fn len_of(&self, pane: Pane) -> usize {
        match pane {
            Pane::Messages => self.visible_messages().len(),
            Pane::Signals => self.current_message().map_or(0, |m| m.signals.len()),
            Pane::Rx => self.rx.len(),
            Pane::Cyclic => self.cyclic.len(),
            Pane::Rules => self.rules.rules.len(),
        }
    }

    pub(super) fn scroll_of(&self, pane: Pane) -> usize {
        self.scroll_ref(pane).offset
    }

    pub(super) fn scroll_ref(&self, pane: Pane) -> &Scroll {
        match pane {
            Pane::Messages => &self.msg_scroll,
            Pane::Signals => &self.sig_scroll,
            Pane::Rx => &self.rx_scroll,
            Pane::Cyclic => &self.cyclic_scroll,
            Pane::Rules => &self.rules_scroll,
        }
    }

    pub(super) fn scroll_mut(&mut self, pane: Pane) -> &mut Scroll {
        match pane {
            Pane::Messages => &mut self.msg_scroll,
            Pane::Signals => &mut self.sig_scroll,
            Pane::Rx => &mut self.rx_scroll,
            Pane::Cyclic => &mut self.cyclic_scroll,
            Pane::Rules => &mut self.rules_scroll,
        }
    }

    /// Visible rows in a pane, as of the last frame. Takes the pane explicitly:
    /// the wheel can scroll one the keyboard is not focused on.
    pub(super) fn page_size(&self, pane: Pane) -> usize {
        let height = self.layout.rect_of(pane).height;
        (height.saturating_sub(pane.header_rows() + 1) as usize).max(1)
    }

    pub(super) fn move_cursor(&mut self, step: i32) {
        let len = self.len_of(self.pane);
        if len == 0 {
            return;
        }
        let cur = match self.pane {
            Pane::Messages => self.msg_index,
            Pane::Signals => self.sig_index,
            Pane::Rx => self.rx_index,
            Pane::Cyclic => self.cyclic_index,
            Pane::Rules => self.rules_index,
        } as i32;
        self.set_cursor((cur + step).clamp(0, len as i32 - 1) as usize);
    }

    pub(super) fn set_cursor(&mut self, index: usize) {
        let len = self.len_of(self.pane);
        if len == 0 {
            return;
        }
        let index = index.min(len - 1);
        self.scroll_mut(self.pane).follow = true;
        match self.pane {
            Pane::Messages => {
                self.msg_index = index;
                self.sig_index = 0; // a different message has different signals
                self.sig_scroll = Scroll {
                    offset: 0,
                    follow: true,
                };
            }
            Pane::Signals => self.sig_index = index,
            Pane::Rx => self.rx_index = index,
            Pane::Cyclic => self.cyclic_index = index,
            Pane::Rules => self.rules_index = index,
        }
    }

    /// The panes Tab can reach right now. The periodic-send panel joins the
    /// cycle only while it is open.
    pub fn panes(&self) -> Vec<Pane> {
        Pane::ALL
            .into_iter()
            .filter(|p| match p {
                Pane::Cyclic => self.show_cyclic_panel,
                Pane::Rules => self.show_rules_panel,
                _ => true,
            })
            .collect()
    }

    pub(super) fn next_pane(&self, step: i32) -> Pane {
        let panes = self.panes();
        let i = panes.iter().position(|p| *p == self.pane).unwrap_or(0) as i32;
        panes[(i + step).rem_euclid(panes.len() as i32) as usize]
    }
}
