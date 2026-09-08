//! All mutable state, and the reducer that changes it.
//!
//! The UI renders from this and the bus thread never sees it. Every mutation
//! enters through [`App::update`] for user intent or [`App::on_bus_event`] for
//! adapter traffic.

pub mod action;
mod cursor;
mod detail;
mod messages;
mod picker;
mod prompt;
mod reactions;
pub mod rx_table;
mod send;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::bus::command::Command;
use crate::bus::event::{Event, Stats};
use crate::config::Config;
use crate::dbc::Database;
use crate::rules::RuleSet;
use crate::transport::Payload;
use crate::transport::spec::TransportSpec;
use crate::ui::layout::LayoutMap;
use action::{Action, Pane, WindowSpot};
pub use detail::{Detail, DetailRow};
use rx_table::RxTable;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptKind {
    Filter,
    SignalValue,
    Period,
    RawSend,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub label: String,
    pub buffer: String,
    /// Shown greyed until the user types, and used when they submit an empty
    /// line. A default must never be pre-filled into `buffer`: typing would
    /// append to it, so `250` over a default of `100` would mean 100250.
    pub default: Option<String>,
}

pub struct PickerRow {
    pub label: String,
    pub detail: String,
}

pub enum PickerKind {
    Interface(Vec<TransportSpec>),
    /// A chosen adapter waiting on a bitrate before we open it.
    Bitrate(TransportSpec),
    Dbc(Vec<PathBuf>),
}

pub struct Picker {
    pub title: String,
    pub rows: Vec<PickerRow>,
    pub index: usize,
    pub kind: PickerKind,
    /// True for the pickers shown on launch, where Esc needs to explain how to
    /// get back to them rather than leave the user staring at an empty screen.
    pub startup: bool,
    /// Set on the launch DBC picker when the interface still needs choosing,
    /// so the two chain. Clear when `--interface` already settled it.
    pub then_interface: bool,
}

pub enum Mode {
    Normal,
    Prompt(Prompt),
    Picker(Picker),
    /// One row shown in full, for when a pane had to cut it short.
    Detail(Detail),
    Help,
}

/// A pane's scroll offset.
///
/// While `follow` is set the view tracks the cursor on every frame. A wheel
/// event clears it so the user can look around without losing their place, and
/// the next keyboard move sets it again.
#[derive(Clone, Copy, Default)]
pub struct Scroll {
    pub offset: usize,
    pub follow: bool,
}

impl Scroll {
    fn to(&mut self, offset: usize) {
        self.offset = offset;
        self.follow = false;
    }
}

/// A half-typed key sequence. Only `gg` needs one today.
/// A periodic send the user has armed. The encoded bytes live here so the
/// panel can show them and so a signal edit can re-arm without re-deriving.
#[derive(Clone)]
pub struct CyclicEntry {
    pub id: embedded_can::Id,
    pub period: Duration,
    pub data: Payload,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Pending {
    #[default]
    None,
    G,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Down,
    /// Dropped, and the bus is reopening it on its own.
    Retrying,
    Up,
}

pub struct App {
    pub db: Arc<Database>,
    /// message name → signal name → physical value.
    pub values: HashMap<String, HashMap<String, f64>>,
    pub rx: RxTable,
    pub stats: Stats,

    pub pane: Pane,
    pub msg_index: usize,
    pub sig_index: usize,
    pub rx_index: usize,
    pub msg_scroll: Scroll,
    pub sig_scroll: Scroll,
    pub rx_scroll: Scroll,
    pub filter: String,

    pub mode: Mode,
    /// A count typed before a motion, e.g. the `12` of `12j`.
    pub count: Option<u32>,
    /// The count as typed, or `None` when the user typed no digits.
    explicit_count: Option<u32>,
    pub pending: Pending,
    /// What `/` replaced, so Esc can put it back.
    filter_before: String,
    pub status: String,
    pub last_error: Option<String>,
    pub layout: LayoutMap,

    pub link: Link,
    pub link_label: String,
    pub spec: Option<TransportSpec>,
    /// Message name → what we armed. Ordered so the panel does not reshuffle.
    /// Mirrors the bus thread's table for display; the bus owns the real one.
    pub cyclic: BTreeMap<String, CyclicEntry>,
    pub show_cyclic_panel: bool,
    pub cyclic_index: usize,
    pub cyclic_scroll: Scroll,

    pub rules: RuleSet,
    pub show_rules_panel: bool,
    pub rules_index: usize,
    pub rules_scroll: Scroll,

    pub config: Config,
    pub mouse: bool,
    pub should_quit: bool,
    commands: Sender<Command>,
}

impl App {
    pub fn new(commands: Sender<Command>, config: Config) -> Self {
        Self {
            db: Arc::new(Database::default()),
            values: HashMap::new(),
            rx: RxTable::default(),
            stats: Stats::default(),
            pane: Pane::Messages,
            msg_index: 0,
            sig_index: 0,
            rx_index: 0,
            msg_scroll: Scroll::default(),
            sig_scroll: Scroll::default(),
            rx_scroll: Scroll::default(),
            filter: String::new(),
            mode: Mode::Normal,
            count: None,
            explicit_count: None,
            pending: Pending::None,
            filter_before: String::new(),
            status: "ready".into(),
            last_error: None,
            layout: LayoutMap::default(),
            link: Link::Down,
            link_label: "not connected".into(),
            spec: None,
            cyclic: BTreeMap::new(),
            show_cyclic_panel: false,
            cyclic_index: 0,
            cyclic_scroll: Scroll::default(),
            rules: RuleSet::default(),
            show_rules_panel: false,
            rules_index: 0,
            rules_scroll: Scroll::default(),
            mouse: config.mouse,
            config,
            should_quit: false,
            commands,
        }
    }

    pub fn on_bus_event(&mut self, event: Event) {
        match event {
            Event::Rx(frame, at) => self.rx.record(&frame, at, &self.db),
            Event::Connected { spec, who, resumed } => {
                self.link = Link::Up;
                self.link_label = who.clone();
                self.status = if resumed {
                    format!("link restored: {who}")
                } else {
                    format!("connected to {who}")
                };
                self.last_error = None;
                self.config.interface = Some(spec.clone());
                if let Some(b) = spec.bitrate() {
                    self.config.bitrate = b;
                }
                self.spec = Some(spec);
                // A resumed link kept its periodic sends; a fresh one did not.
                if !resumed {
                    self.cyclic.clear();
                }
            }
            Event::ConnectFailed(e) => {
                self.link = Link::Down;
                self.link_label = "not connected".into();
                self.last_error = Some(e.clone());
                self.status = e;
            }
            Event::Disconnected { retrying } => {
                if retrying {
                    self.link = Link::Retrying;
                    self.link_label = "link lost, reconnecting".into();
                    self.status = "link lost, retrying. Periodic sends held".into();
                } else {
                    self.link = Link::Down;
                    self.link_label = "not connected".into();
                    self.cyclic.clear();
                    self.status = "disconnected".into();
                }
            }
            Event::Reconnecting { attempt } => {
                self.link = Link::Retrying;
                self.link_label = "link lost, reconnecting".into();
                self.status = format!("reconnecting… (attempt {attempt})");
            }
            Event::BusError(e) => self.last_error = Some(e),
            Event::Stats(s) => self.stats = s,
        }
    }

    pub fn update(&mut self, action: Action) {
        match &self.mode {
            Mode::Prompt(_) => return self.update_prompt(action),
            Mode::Picker(_) => return self.update_picker(action),
            Mode::Detail(_) => return self.update_detail(action),
            Mode::Help => {
                if !matches!(action, Action::None) {
                    self.mode = Mode::Normal;
                }
                return;
            }
            Mode::Normal => {}
        }

        // Every action except a count digit ends a count, and every action
        // except the second `g` ends a pending sequence.
        if !matches!(action, Action::CountDigit(_)) {
            self.pending = Pending::None;
        }

        match action {
            Action::CountDigit(d) => {
                // A leading zero is not a count, so `0` stays free.
                if d != 0 || self.count.is_some() {
                    let next = self.count.unwrap_or(0).saturating_mul(10) + d;
                    self.count = Some(next.min(100_000));
                }
                return;
            }
            Action::BeginG => {
                self.pending = Pending::G;
                return;
            }
            _ => {}
        }
        let count = self.take_count();

        match action {
            Action::None => {}
            Action::Quit => self.quit(),
            Action::NextPane => self.pane = self.next_pane(1),
            Action::PrevPane => self.pane = self.next_pane(-1),
            Action::Move(step) => self.move_cursor(step * count),
            Action::Page(step) => self.move_cursor(step * self.page_size(self.pane) as i32 * count),
            Action::HalfPage(step) => {
                let half = (self.page_size(self.pane) / 2).max(1) as i32;
                self.move_cursor(step * half * count);
            }
            Action::Home => self.set_cursor(0),
            // `G` alone goes to the end; `42G` goes to row 42, as in vim.
            Action::End => match self.explicit_count {
                Some(n) => self.set_cursor((n as usize).saturating_sub(1)),
                None => self.set_cursor(usize::MAX),
            },
            Action::Window(spot) => {
                let top = self.scroll_of(self.pane);
                let rows = self.page_size(self.pane);
                let last = self.len_of(self.pane).saturating_sub(1);
                let target = match spot {
                    WindowSpot::Top => top,
                    WindowSpot::Middle => top + rows / 2,
                    WindowSpot::Bottom => top + rows.saturating_sub(1),
                };
                self.set_cursor(target.min(last));
            }
            Action::FocusAndSelect(pane, row) => {
                self.pane = pane;
                let row = self.scroll_of(pane) + row;
                if row < self.len_of(pane) {
                    self.set_cursor(row);
                }
            }
            Action::ScrollIn(pane, step) => {
                // Deliberately does not move focus: a stray wheel event must
                // never displace where the keyboard is.
                let pane = pane.unwrap_or(self.pane);
                let len = self.len_of(pane);
                let page = self.page_size(pane);
                let max = len.saturating_sub(page);
                let cur = self.scroll_of(pane) as i32;
                let next = (cur + step).clamp(0, max as i32) as usize;
                self.scroll_mut(pane).to(next);
            }

            Action::BeginFilter => {
                // Start empty: re-filtering is nearly always a fresh search, and
                // editing the old text meant cancelling and retyping anyway.
                self.filter_before = std::mem::take(&mut self.filter);
                self.msg_index = 0;
                self.msg_scroll = Scroll {
                    offset: 0,
                    follow: true,
                };
                self.prompt(PromptKind::Filter, "filter:".into(), String::new());
            }
            Action::Inspect => self.inspect(),
            Action::BeginEditSignal => {
                // The receive table has nothing to edit, so enter shows the row
                // in full instead, which is what you want there anyway.
                if self.pane == Pane::Rx {
                    self.inspect();
                } else if self.pane == Pane::Rules {
                    self.toggle_selected_rule();
                } else if self.pane == Pane::Cyclic {
                    // Enter on a periodic send jumps to the message it sends,
                    // which is where you would go to change it anyway.
                    self.jump_to_selected_cyclic();
                } else {
                    self.begin_edit_signal();
                }
            }
            Action::ToggleCyclicPanel => {
                self.show_cyclic_panel = !self.show_cyclic_panel;
                if !self.show_cyclic_panel && self.pane == Pane::Cyclic {
                    self.pane = Pane::Messages;
                }
                self.status = if self.show_cyclic_panel {
                    "cyclic panel open: tab to focus, d stops one, enter jumps to it".into()
                } else {
                    "cyclic panel closed".into()
                };
            }
            Action::StopSelectedCyclic => self.stop_selected_cyclic(),
            Action::ToggleRulesPanel => {
                self.show_rules_panel = !self.show_rules_panel;
                if !self.show_rules_panel && self.pane == Pane::Rules {
                    self.pane = Pane::Messages;
                }
                self.status = if self.show_rules_panel {
                    format!(
                        "rules panel open: {} loaded, enter toggles one, F7 reloads",
                        self.rules.rules.len()
                    )
                } else {
                    "rules panel closed".into()
                };
            }
            Action::ReloadRules => self.reload_rules(),
            Action::SendOnce => self.send_once(),
            Action::ToggleCyclic => self.toggle_cyclic(),
            Action::StopAllCyclic => {
                self.send_command(Command::ClearAllCyclic);
                self.cyclic.clear();
                self.status = "all cyclic sends stopped".into();
            }
            Action::BeginRawSend => self.prompt(
                PromptKind::RawSend,
                "raw  <hex id> <hex bytes>:".into(),
                String::new(),
            ),
            Action::ClearRx => {
                self.rx.clear();
                self.rx_index = 0;
                self.rx_scroll = Scroll {
                    offset: 0,
                    follow: true,
                };
                self.status = "receive table cleared".into();
            }

            Action::OpenInterfacePicker => self.open_interface_picker(false),
            Action::OpenDbcPicker => self.open_dbc_picker(false),
            Action::ToggleMouse => {
                self.mouse = !self.mouse;
                self.config.mouse = self.mouse;
                self.status = format!(
                    "mouse {}, {}",
                    if self.mouse { "on" } else { "off" },
                    if self.mouse {
                        "wheel scrolls, click selects"
                    } else {
                        "terminal text selection works again"
                    }
                );
            }
            Action::ToggleHelp => self.mode = Mode::Help,
            Action::Cancel => {}
            _ => {}
        }
    }

    fn quit(&mut self) {
        self.send_command(Command::ClearAllCyclic);
        self.send_command(Command::Shutdown);
        self.should_quit = true;
    }

    fn send_command(&self, cmd: Command) {
        let _ = self.commands.send(cmd);
    }

    /// Consume the pending count, defaulting to 1. `explicit_count` keeps the
    /// raw value for the motions that treat "no count" differently from "1".
    fn take_count(&mut self) -> i32 {
        self.explicit_count = self.count.take();
        self.explicit_count.unwrap_or(1).clamp(1, 100_000) as i32
    }

    pub fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Short decimal rendering, matching what the signal pane shows.
pub fn trim(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.6}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{self, Act, Condition, Op, Rule, Source};

    fn app() -> App {
        let (tx, _rx) = crossbeam_channel::unbounded();
        App::new(tx, Config::default())
    }

    fn flip(name: &str, from: f64, to: f64) -> Rule {
        Rule {
            name: name.into(),
            enabled: true,
            all: vec![Condition {
                source: Source::Tx,
                message: "M".into(),
                signal: "a".into(),
                op: Op::Eq,
                value: from,
                seen: None,
            }],
            any: Vec::new(),
            then: vec![Act::Set {
                message: "M".into(),
                signal: "a".into(),
                value: to,
            }],
            fired: 0,
            last_fired: None,
            warning: None,
            holding: false,
        }
    }

    /// Rules can chain, so they can also form a cycle. Evaluation has to stop
    /// and say so rather than run until the frame budget is gone.
    #[test]
    fn a_rule_cycle_stops_instead_of_spinning() {
        let mut app = app();
        app.rules.rules = vec![flip("up", 0.0, 1.0), flip("down", 1.0, 0.0)];
        app.values
            .insert("M".into(), HashMap::from([("a".to_string(), 0.0)]));

        app.run_rules();

        assert!(app.rules.looped, "a cycle should be reported");
        assert!(
            app.last_error
                .as_deref()
                .is_some_and(|e| e.contains("loop")),
            "the user should be told: {:?}",
            app.last_error
        );
        // Bounded, not unbounded: each rule fired once per pass, no more.
        let total: u64 = app.rules.rules.iter().map(|r| r.fired).sum();
        assert!(total <= rules::MAX_PASSES as u64 + 1, "fired {total} times");
    }

    /// The ordinary case must not be mistaken for a cycle.
    #[test]
    fn a_settling_chain_does_not_report_a_loop() {
        let mut app = app();
        app.rules.rules = vec![flip("once", 0.0, 1.0)];
        app.values
            .insert("M".into(), HashMap::from([("a".to_string(), 0.0)]));

        app.run_rules();

        assert!(!app.rules.looped);
        assert_eq!(app.rules.rules[0].fired, 1);
        assert_eq!(app.values["M"]["a"], 1.0);
    }
}
