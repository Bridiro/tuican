//! Conditional reactions: "change this message to this if this message changes
//! to that".
//!
//! A rule watches signals and, when its condition becomes true, performs
//! actions. Rules are independent, so any number of them can react to the same
//! frame. A rule can also watch a signal we are sending, letting one rule's
//! action satisfy another rule's condition. Together those give a graph of
//! reactions rather than a list, and [`MAX_PASSES`] bounds a cycle in it.

pub mod load;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use serde::Deserialize;

use crate::app::rx_table::RxTable;

/// How many times a single evaluation may cascade before we stop and say so.
/// A rule that re-arms its own condition would otherwise never return.
pub const MAX_PASSES: usize = 8;

/// Where a condition reads its value from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A signal on a message arriving from the bus.
    #[default]
    Rx,
    /// A signal on a message we are sending, which is how rules chain.
    Tx,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    #[default]
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// True on any change of value, whatever it changed to.
    Changed,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Changed => "changed",
        }
    }
}

/// Floating point equality on decoded signals needs a tolerance: a value that
/// went through scale and offset rarely lands exactly on the number you typed.
const EPSILON: f64 = 1e-9;

#[derive(Clone, Debug)]
pub struct Condition {
    pub source: Source,
    pub message: String,
    pub signal: String,
    pub op: Op,
    pub value: f64,
    /// Previous reading, for `changed`.
    pub seen: Option<f64>,
}

impl Condition {
    fn holds(&mut self, current: Option<f64>) -> bool {
        let Some(v) = current else {
            // A message we have never received cannot satisfy a condition.
            self.seen = None;
            return false;
        };
        let result = match self.op {
            Op::Eq => (v - self.value).abs() <= EPSILON,
            Op::Ne => (v - self.value).abs() > EPSILON,
            Op::Lt => v < self.value,
            Op::Le => v <= self.value,
            Op::Gt => v > self.value,
            Op::Ge => v >= self.value,
            Op::Changed => self.seen.is_some_and(|p| (v - p).abs() > EPSILON),
        };
        self.seen = Some(v);
        result
    }

    pub fn describe(&self) -> String {
        let scope = match self.source {
            Source::Rx => "",
            Source::Tx => "tx:",
        };
        if self.op == Op::Changed {
            format!("{scope}{}.{} changed", self.message, self.signal)
        } else {
            format!(
                "{scope}{}.{} {} {}",
                self.message,
                self.signal,
                self.op.symbol(),
                crate::app::trim(self.value)
            )
        }
    }
}

/// What a rule does when it fires.
#[derive(Clone, Debug)]
pub enum Act {
    /// Change a signal on a message we send.
    Set {
        message: String,
        signal: String,
        value: f64,
    },
    /// Send a message once.
    Send { message: String },
    /// Start (or re-arm) a periodic send.
    Cyclic { message: String, period_ms: f64 },
    /// Stop a periodic send.
    Stop { message: String },
}

impl Act {
    pub fn describe(&self) -> String {
        match self {
            Act::Set {
                message,
                signal,
                value,
            } => {
                format!("set {message}.{signal} = {}", crate::app::trim(*value))
            }
            Act::Send { message } => format!("send {message}"),
            Act::Cyclic { message, period_ms } => {
                format!("cyclic {message} every {} ms", crate::app::trim(*period_ms))
            }
            Act::Stop { message } => format!("stop {message}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub name: String,
    pub enabled: bool,
    /// Every one of these must hold.
    pub all: Vec<Condition>,
    /// At least one of these must hold, when the list is not empty.
    pub any: Vec<Condition>,
    pub then: Vec<Act>,
    pub fired: u64,
    pub last_fired: Option<Instant>,
    /// Why this rule cannot work, when the loader found something wrong with
    /// it. Shown in the panel: a rule that silently never fires is the worst
    /// way to spend an hour.
    pub warning: Option<String>,
    /// Whether the condition held last time we looked. Rules fire on the rising
    /// edge; without this a rule watching `state == 2` would fire on every
    /// frame for as long as the state stayed at 2.
    pub holding: bool,
}

impl Rule {
    fn conditions_hold(&mut self, rx: &RxTable, tx: &TxValues) -> bool {
        let lookup = |c: &Condition| match c.source {
            Source::Rx => rx.signal(&c.message, &c.signal),
            Source::Tx => tx.get(&c.message).and_then(|m| m.get(&c.signal)).copied(),
        };
        // Evaluate every condition, even once the answer is known: `changed`
        // has to see each frame or it would miss transitions.
        let mut all_hold = true;
        for c in &mut self.all {
            let v = lookup(c);
            if !c.holds(v) {
                all_hold = false;
            }
        }
        let mut any_holds = self.any.is_empty();
        for c in &mut self.any {
            let v = lookup(c);
            if c.holds(v) {
                any_holds = true;
            }
        }
        all_hold && any_holds && !(self.all.is_empty() && self.any.is_empty())
    }

    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = self.all.iter().map(Condition::describe).collect();
        if !self.any.is_empty() {
            let any: Vec<String> = self.any.iter().map(Condition::describe).collect();
            parts.push(format!("any({})", any.join(", ")));
        }
        let then: Vec<String> = self.then.iter().map(Act::describe).collect();
        format!("when {} then {}", parts.join(" and "), then.join(", "))
    }
}

pub type TxValues = HashMap<String, HashMap<String, f64>>;

#[derive(Default)]
pub struct RuleSet {
    pub path: Option<PathBuf>,
    pub rules: Vec<Rule>,
    /// Names in the file that the loaded DBC does not define. Shown rather than
    /// silently never matching, which is the failure mode that wastes an hour.
    pub warnings: Vec<String>,
    /// Set when a cascade hit [`MAX_PASSES`], so the UI can say the graph loops.
    pub looped: bool,
}

impl RuleSet {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn label(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(no rules)".into())
    }

    /// One pass: every enabled rule whose condition just became true.
    ///
    /// Returns the actions to apply. Applying them is the caller's job, because
    /// the caller owns the transmit values and the bus channel.
    pub fn pass(&mut self, rx: &RxTable, tx: &TxValues) -> Vec<(String, Vec<Act>)> {
        let now = Instant::now();
        let mut fired = Vec::new();
        for rule in &mut self.rules {
            if !rule.enabled {
                rule.holding = false;
                continue;
            }
            let holds = rule.conditions_hold(rx, tx);
            if holds && !rule.holding {
                rule.fired += 1;
                rule.last_fired = Some(now);
                fired.push((rule.name.clone(), rule.then.clone()));
            }
            rule.holding = holds;
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(message: &str, signal: &str, op: Op, value: f64) -> Condition {
        Condition {
            source: Source::Rx,
            message: message.into(),
            signal: signal.into(),
            op,
            value,
            seen: None,
        }
    }

    fn rule(name: &str, all: Vec<Condition>) -> Rule {
        Rule {
            name: name.into(),
            enabled: true,
            all,
            any: Vec::new(),
            then: vec![Act::Send {
                message: "Out".into(),
            }],
            fired: 0,
            last_fired: None,
            warning: None,
            holding: false,
        }
    }

    #[test]
    fn a_rule_fires_on_the_edge_not_on_every_frame() {
        let mut set = RuleSet {
            rules: vec![rule("r", vec![cond("In", "state", Op::Eq, 2.0)])],
            ..Default::default()
        };
        let tx = TxValues::new();
        let mut rx = RxTable::default();

        rx.set_for_test("In", "state", 1.0);
        assert!(set.pass(&rx, &tx).is_empty());

        rx.set_for_test("In", "state", 2.0);
        assert_eq!(
            set.pass(&rx, &tx).len(),
            1,
            "should fire when it becomes true"
        );
        assert!(
            set.pass(&rx, &tx).is_empty(),
            "must not fire again while it stays true"
        );

        // Falling and rising again arms it once more.
        rx.set_for_test("In", "state", 0.0);
        assert!(set.pass(&rx, &tx).is_empty());
        rx.set_for_test("In", "state", 2.0);
        assert_eq!(set.pass(&rx, &tx).len(), 1);
        assert_eq!(set.rules[0].fired, 2);
    }

    #[test]
    fn changed_fires_on_any_transition() {
        let mut set = RuleSet {
            rules: vec![rule("r", vec![cond("In", "counter", Op::Changed, 0.0)])],
            ..Default::default()
        };
        let tx = TxValues::new();
        let mut rx = RxTable::default();

        rx.set_for_test("In", "counter", 5.0);
        assert!(
            set.pass(&rx, &tx).is_empty(),
            "first sighting is not a change"
        );
        rx.set_for_test("In", "counter", 6.0);
        assert_eq!(set.pass(&rx, &tx).len(), 1);
    }

    #[test]
    fn a_message_never_received_never_fires() {
        let mut set = RuleSet {
            rules: vec![rule("r", vec![cond("Absent", "x", Op::Ne, 99.0)])],
            ..Default::default()
        };
        assert!(set.pass(&RxTable::default(), &TxValues::new()).is_empty());
    }

    #[test]
    fn every_matching_rule_fires_from_one_frame() {
        let mut set = RuleSet {
            rules: vec![
                rule("a", vec![cond("In", "state", Op::Eq, 2.0)]),
                rule("b", vec![cond("In", "state", Op::Ge, 1.0)]),
                rule("c", vec![cond("In", "state", Op::Eq, 9.0)]),
            ],
            ..Default::default()
        };
        let mut rx = RxTable::default();
        rx.set_for_test("In", "state", 2.0);
        let fired = set.pass(&rx, &TxValues::new());
        assert_eq!(fired.len(), 2, "parallel branches both fire: {fired:?}");
    }

    #[test]
    fn a_disabled_rule_does_not_fire_and_rearms_when_enabled() {
        let mut set = RuleSet {
            rules: vec![rule("r", vec![cond("In", "state", Op::Eq, 1.0)])],
            ..Default::default()
        };
        set.rules[0].enabled = false;
        let mut rx = RxTable::default();
        rx.set_for_test("In", "state", 1.0);
        assert!(set.pass(&rx, &TxValues::new()).is_empty());

        set.rules[0].enabled = true;
        assert_eq!(set.pass(&rx, &TxValues::new()).len(), 1);
    }

    #[test]
    fn a_tx_condition_lets_one_rule_follow_another() {
        let mut set = RuleSet {
            rules: vec![Rule {
                all: vec![Condition {
                    source: Source::Tx,
                    ..cond("Out", "enable", Op::Eq, 1.0)
                }],
                ..rule("follower", vec![])
            }],
            ..Default::default()
        };
        let mut tx = TxValues::new();
        assert!(set.pass(&RxTable::default(), &tx).is_empty());
        tx.entry("Out".into())
            .or_default()
            .insert("enable".into(), 1.0);
        assert_eq!(set.pass(&RxTable::default(), &tx).len(), 1);
    }
}
