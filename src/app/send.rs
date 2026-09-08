//! Sending frames, once or periodically.

use super::action::Pane;
use super::{App, CyclicEntry, Link, PromptKind};
use crate::bus::command::Command;

impl App {
    pub(super) fn send_once(&mut self) {
        if self.link != Link::Up {
            self.status = "not connected, press F4 to pick an interface".into();
            return;
        }
        if let Some((name, id, data)) = self.encode_current() {
            self.send_command(Command::Send { id, data });
            self.status = format!("sent {name}  {}", data.hex());
        }
    }

    pub(super) fn toggle_cyclic(&mut self) {
        let Some(msg) = self.current_message().cloned() else {
            return;
        };
        if self.cyclic.contains_key(&msg.name) {
            self.send_command(Command::ClearCyclic(msg.name.clone()));
            self.cyclic.remove(&msg.name);
            self.status = format!("stopped {}", msg.name);
            return;
        }
        if self.link == Link::Down {
            self.status = "not connected, press F4 to pick an interface".into();
            return;
        }
        if self.encode_current().is_none() {
            return; // status already explains why
        }
        let default = msg.cycle_time_ms.unwrap_or(100);
        self.prompt_with_default(
            PromptKind::Period,
            format!("period ms for {}:", msg.name),
            default.to_string(),
        );
    }

    /// Push new bytes into a periodic send that is already running.
    pub(super) fn rearm_cyclic(&mut self, message: &str) {
        let Some(period) = self.cyclic.get(message).map(|e| e.period) else {
            return;
        };
        if let Some((id, data)) = self.encode_named(message) {
            self.cyclic
                .insert(message.to_string(), CyclicEntry { id, period, data });
            self.send_command(Command::SetCyclic {
                key: message.to_string(),
                id,
                data,
                period,
            });
        }
    }

    pub(super) fn stop_selected_cyclic(&mut self) {
        let Some(name) = self.selected_cyclic() else {
            return;
        };
        self.send_command(Command::ClearCyclic(name.clone()));
        self.cyclic.remove(&name);
        self.cyclic_index = self.cyclic_index.min(self.cyclic.len().saturating_sub(1));
        self.status = format!("stopped {name}");
    }

    /// Put the message cursor on the selected periodic send, clearing any
    /// filter that would otherwise hide it.
    pub(super) fn jump_to_selected_cyclic(&mut self) {
        let Some(name) = self.selected_cyclic() else {
            return;
        };
        if !self.visible_messages().iter().any(|m| m.name == name) {
            self.filter.clear();
        }
        if let Some(i) = self.visible_messages().iter().position(|m| m.name == name) {
            self.pane = Pane::Messages;
            self.set_cursor(i);
            self.status = format!("jumped to {name}");
        }
    }

    /// Name of the periodic send under the cursor in the panel.
    pub fn selected_cyclic(&self) -> Option<String> {
        self.cyclic.keys().nth(self.cyclic_index).cloned()
    }
}
