//! Loading reaction rules and applying what they fire.

use std::time::Duration;

use super::{App, CyclicEntry};
use crate::bus::command::Command;
use crate::rules::{self, Act};

impl App {
    /// Load a rules file, replacing whatever was loaded. As with the DBC, a
    /// failure leaves the previous set in place.
    pub fn load_rules(&mut self, path: &std::path::Path) {
        match rules::load::load(path, &self.db) {
            Ok(set) => {
                let count = set.rules.len();
                let warnings = set.warnings.clone();
                self.rules = set;
                self.config.rules = Some(path.to_path_buf());
                self.status = format!("loaded {count} rules from {}", self.rules.label());
                self.last_error = warnings.first().map(|w| {
                    if warnings.len() > 1 {
                        format!("{w} (and {} more)", warnings.len() - 1)
                    } else {
                        w.clone()
                    }
                });
            }
            Err(e) => {
                self.status = format!("{e:#}");
                self.last_error = Some(format!("{e:#}"));
            }
        }
    }

    pub(super) fn reload_rules(&mut self) {
        match self.rules.path.clone() {
            Some(path) => self.load_rules(&path),
            None => self.status = "no rules file loaded, start tuican with --rules FILE".into(),
        }
    }

    pub(super) fn toggle_selected_rule(&mut self) {
        let Some(rule) = self.rules.rules.get_mut(self.rules_index) else {
            return;
        };
        rule.enabled = !rule.enabled;
        // A rule re-enabled mid-run must not fire on a condition that was
        // already true before it was switched back on.
        rule.holding = false;
        self.status = format!(
            "rule {} {}",
            rule.name,
            if rule.enabled { "enabled" } else { "disabled" }
        );
    }

    /// Evaluate the rules and apply what fires.
    ///
    /// Repeats until nothing new fires, so a rule that reacts to a signal
    /// another rule just set runs in the same tick rather than a frame later.
    pub fn run_rules(&mut self) {
        if self.rules.is_empty() {
            return;
        }
        for pass in 0..rules::MAX_PASSES {
            let fired = self.rules.pass(&self.rx, &self.values);
            if fired.is_empty() {
                self.rules.looped = false;
                return;
            }
            for (name, actions) in fired {
                for act in actions {
                    self.apply_act(&name, &act);
                }
            }
            if pass + 1 == rules::MAX_PASSES {
                // A cycle in the graph. Say so once rather than spin forever.
                if !self.rules.looped {
                    self.last_error = Some(format!(
                        "rules did not settle after {} passes; check for a loop",
                        rules::MAX_PASSES
                    ));
                }
                self.rules.looped = true;
            }
        }
    }

    pub(super) fn apply_act(&mut self, rule: &str, act: &Act) {
        match act {
            Act::Set {
                message,
                signal,
                value,
            } => {
                self.values_for(message).insert(signal.clone(), *value);
                self.rearm_cyclic(message);
                self.status = format!("{rule}: {}", act.describe());
            }
            Act::Send { message } => {
                if let Some((id, data)) = self.encode_named(message) {
                    self.send_command(Command::Send { id, data });
                    self.status = format!("{rule}: {}", act.describe());
                }
            }
            Act::Cyclic { message, period_ms } => {
                if *period_ms <= 0.0 {
                    return;
                }
                if let Some((id, data)) = self.encode_named(message) {
                    let period = Duration::from_secs_f64(period_ms / 1000.0);
                    self.send_command(Command::SetCyclic {
                        key: message.clone(),
                        id,
                        data,
                        period,
                    });
                    self.cyclic
                        .insert(message.clone(), CyclicEntry { id, period, data });
                    self.status = format!("{rule}: {}", act.describe());
                }
            }
            Act::Stop { message } => {
                if self.cyclic.remove(message).is_some() {
                    self.send_command(Command::ClearCyclic(message.clone()));
                    self.status = format!("{rule}: {}", act.describe());
                }
            }
        }
    }
}
