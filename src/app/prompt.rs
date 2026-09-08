//! The single-line prompt used for filters, values, periods and raw frames.

use std::time::Duration;

use super::action::Action;
use super::{App, CyclicEntry, Mode, Prompt, PromptKind, Scroll, trim};
use crate::bus::command::Command;
use crate::dbc::mux;
use crate::transport::Payload;

impl App {
    pub(super) fn begin_edit_signal(&mut self) {
        let Some(msg) = self.current_message().cloned() else {
            return;
        };
        let Some(sig) = msg
            .signals
            .get(self.sig_index.min(msg.signals.len().saturating_sub(1)))
        else {
            return;
        };
        // Show what the field can actually hold, so an out-of-range entry is
        // avoidable rather than merely reported.
        let hint = if !sig.choices.is_empty() {
            let opts: Vec<String> = sig
                .choices
                .iter()
                .take(6)
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            format!(" ({})", opts.join(", "))
        } else {
            let (lo, hi) = mux::effective_range(sig);
            format!(" [{} .. {}]", trim(lo), trim(hi))
        };
        let current = self
            .values_of(&msg.name)
            .get(&sig.name)
            .copied()
            .unwrap_or(0.0);
        self.prompt_with_default(
            PromptKind::SignalValue,
            format!("{}{hint} =", sig.name),
            trim(current),
        );
    }

    pub(super) fn prompt(&mut self, kind: PromptKind, label: String, buffer: String) {
        self.mode = Mode::Prompt(Prompt {
            kind,
            label,
            buffer,
            default: None,
        });
    }

    pub(super) fn prompt_with_default(&mut self, kind: PromptKind, label: String, default: String) {
        self.mode = Mode::Prompt(Prompt {
            kind,
            label,
            buffer: String::new(),
            default: Some(default),
        });
    }

    pub(super) fn update_prompt(&mut self, action: Action) {
        let Mode::Prompt(prompt) = &mut self.mode else {
            return;
        };
        match action {
            Action::PromptChar(c) => {
                prompt.buffer.push(c);
                // The filter is live: no need to press enter to see it work.
                if prompt.kind == PromptKind::Filter {
                    self.apply_filter();
                }
            }
            Action::PromptBackspace => {
                prompt.buffer.pop();
                if prompt.kind == PromptKind::Filter {
                    self.apply_filter();
                }
            }
            Action::Cancel => {
                let was_filter = prompt.kind == PromptKind::Filter;
                self.mode = Mode::Normal;
                if was_filter {
                    // Esc undoes the `/`, rather than also clearing a filter the
                    // user had deliberately set earlier.
                    self.filter = std::mem::take(&mut self.filter_before);
                    self.msg_index = 0;
                }
            }
            Action::PromptSubmit => self.submit_prompt(),
            _ => {}
        }
    }

    pub(super) fn apply_filter(&mut self) {
        let Mode::Prompt(p) = &self.mode else { return };
        self.filter = p.buffer.clone();
        self.msg_index = 0;
        self.msg_scroll = Scroll {
            offset: 0,
            follow: true,
        };
    }

    pub(super) fn submit_prompt(&mut self) {
        let Mode::Prompt(prompt) = &self.mode else {
            return;
        };
        let typed = prompt.buffer.trim();
        let kind = prompt.kind;
        let text = if typed.is_empty() {
            prompt.default.clone().unwrap_or_default()
        } else {
            typed.to_string()
        };
        self.mode = Mode::Normal;
        match kind {
            PromptKind::Filter => {
                self.filter = text;
                self.msg_index = 0;
            }
            PromptKind::SignalValue => self.commit_signal_value(&text),
            PromptKind::Period => self.commit_period(&text),
            PromptKind::RawSend => self.commit_raw_send(&text),
        }
    }

    pub(super) fn commit_signal_value(&mut self, text: &str) {
        let Some(msg) = self.current_message().cloned() else {
            return;
        };
        let Some(sig) = msg
            .signals
            .get(self.sig_index.min(msg.signals.len().saturating_sub(1)))
        else {
            return;
        };
        let value = match text.parse::<f64>() {
            Ok(v) => v,
            // Fall back to matching an enum name, so you can type `ACTIVE`.
            Err(_) => {
                let hit = sig
                    .choices
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(text));
                match hit {
                    Some((&raw, _)) => raw as f64 * sig.factor + sig.offset,
                    None => {
                        self.status = format!("cannot read {text:?} as a number or a value name");
                        return;
                    }
                }
            }
        };
        let (lo, hi) = mux::representable_range(sig);
        if value < lo || value > hi {
            self.status = format!(
                "{} = {} does not fit; the field holds [{} .. {}]",
                sig.name,
                trim(value),
                trim(lo),
                trim(hi)
            );
            return;
        }
        let (name, sig_name) = (msg.name.clone(), sig.name.clone());
        self.values_for(&name).insert(sig_name.clone(), value);
        self.status = format!("{sig_name} = {}", trim(value));

        // Re-arm an already-running cyclic send with the new bytes, otherwise
        // the edit silently does nothing until you toggle it off and on.
        if let Some(period) = self.cyclic.get(&name).map(|e| e.period)
            && let Some((_, id, data)) = self.encode_current()
        {
            self.cyclic
                .insert(name.clone(), CyclicEntry { id, period, data });
            self.send_command(Command::SetCyclic {
                key: name,
                id,
                data,
                period,
            });
        }
    }

    pub(super) fn commit_period(&mut self, text: &str) {
        let Ok(ms) = text.parse::<f64>() else {
            self.status = format!("{text:?} is not a period in milliseconds");
            return;
        };
        if ms <= 0.0 {
            self.status = "period must be greater than zero".into();
            return;
        }
        let Some((name, id, data)) = self.encode_current() else {
            return;
        };
        let period = Duration::from_secs_f64(ms / 1000.0);
        self.send_command(Command::SetCyclic {
            key: name.clone(),
            id,
            data,
            period,
        });
        self.cyclic
            .insert(name.clone(), CyclicEntry { id, period, data });
        self.status = format!("sending {name} every {} ms", trim(ms));
    }

    pub(super) fn commit_raw_send(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let parts: Vec<&str> = text
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        let Some((id_text, rest)) = parts.split_first() else {
            return;
        };
        let Ok(raw) = u32::from_str_radix(id_text.trim_start_matches("0x"), 16) else {
            self.status = format!("{id_text:?} is not a hex id");
            return;
        };
        let mut data = Vec::new();
        for byte in rest {
            match u8::from_str_radix(byte, 16) {
                Ok(b) => data.push(b),
                Err(_) => {
                    self.status = format!("{byte:?} is not a hex byte");
                    return;
                }
            }
        }
        if data.len() > 8 {
            self.status = "classic CAN carries at most 8 bytes".into();
            return;
        }
        let Some(id) = crate::canid::make(raw, raw > 0x7FF) else {
            self.status = format!("{raw:#X} is not a valid CAN id");
            return;
        };
        let payload = Payload::new(&data);
        self.send_command(Command::Send { id, data: payload });
        self.status = format!("sent raw {} {}", crate::canid::display(id), payload.hex());
    }
}
