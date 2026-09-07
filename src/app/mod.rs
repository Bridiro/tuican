//! All mutable state, and the reducer that changes it.
//!
//! The UI is a pure function of this; the bus thread never sees it. Every
//! mutation enters through [`App::update`] (user intent) or
//! [`App::on_bus_event`] (adapter traffic).

pub mod action;
pub mod rx_table;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::bus::command::Command;
use crate::bus::event::{Event, Stats};
use crate::config::Config;
use crate::dbc::{self, Database, MessageDef, codec, mux};
use crate::rules::{self, Act, RuleSet};
use crate::transport::spec::{COMMON_BITRATES, TransportSpec};
use crate::transport::{Payload, discover};
use crate::ui::layout::LayoutMap;
use action::{Action, Pane, WindowSpot};
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
    Help,
}

/// A pane's scroll offset.
///
/// `follow` is what keeps the wheel from fighting the keyboard: while it is
/// set, the view tracks the cursor every frame; a wheel event clears it so the
/// user can look around without losing their place, and the next keyboard move
/// re-attaches.
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

    // ---- messages and signals -------------------------------------------

    pub fn visible_messages(&self) -> Vec<&MessageDef> {
        if self.filter.is_empty() {
            return self.db.messages.iter().collect();
        }
        let needle = self.filter.to_lowercase();
        self.db
            .messages
            .iter()
            .filter(|m| {
                m.name.to_lowercase().contains(&needle)
                    || crate::canid::display(m.id).to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn current_message(&self) -> Option<&MessageDef> {
        let shown = self.visible_messages();
        shown.get(self.msg_index.min(shown.len().saturating_sub(1))).copied()
    }

    /// Values for a message, seeded on first use with something that encodes.
    fn values_for(&mut self, name: &str) -> &mut HashMap<String, f64> {
        if !self.values.contains_key(name) {
            let seed = self
                .db
                .messages
                .iter()
                .find(|m| m.name == name)
                .map(mux::initial_values)
                .unwrap_or_default();
            self.values.insert(name.to_string(), seed);
        }
        self.values.get_mut(name).expect("just inserted")
    }

    pub fn values_of(&self, name: &str) -> HashMap<String, f64> {
        self.values.get(name).cloned().unwrap_or_else(|| {
            self.db
                .messages
                .iter()
                .find(|m| m.name == name)
                .map(mux::initial_values)
                .unwrap_or_default()
        })
    }

    fn encode_current(&mut self) -> Option<(String, embedded_can::Id, Payload)> {
        let name = self.current_message()?.name.clone();
        let (id, data) = self.encode_named(&name)?;
        Some((name, id, data))
    }

    /// Encode any message by name, which is what the rules need: they act on
    /// messages the cursor is nowhere near.
    fn encode_named(&mut self, name: &str) -> Option<(embedded_can::Id, Payload)> {
        let msg = self.db.messages.iter().find(|m| m.name == name)?.clone();
        let values = self.values_of(name);
        match codec::encode(&msg, &values) {
            Ok(data) => Some((msg.id, Payload::new(&data))),
            Err(e) => {
                self.status = format!("cannot encode {name}: {e}");
                None
            }
        }
    }

    // ---- events from the bus --------------------------------------------

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
                    self.status = "link lost — retrying, periodic sends held".into();
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

    // ---- the reducer ------------------------------------------------------

    pub fn update(&mut self, action: Action) {
        match &self.mode {
            Mode::Prompt(_) => return self.update_prompt(action),
            Mode::Picker(_) => return self.update_picker(action),
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
            Action::Page(step) => {
                self.move_cursor(step * self.page_size(self.pane) as i32 * count)
            }
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
                self.msg_scroll = Scroll { offset: 0, follow: true };
                self.prompt(PromptKind::Filter, "filter:".into(), String::new());
            }
            Action::BeginEditSignal => {
                if self.pane == Pane::Rules {
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
                    "cyclic panel open — tab to focus, d stops one, enter jumps to it".into()
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
                        "rules panel open — {} loaded, enter toggles one, F7 reloads",
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
                self.rx_scroll = Scroll { offset: 0, follow: true };
                self.status = "receive table cleared".into();
            }

            Action::OpenInterfacePicker => self.open_interface_picker(false),
            Action::OpenDbcPicker => self.open_dbc_picker(false),
            Action::ToggleMouse => {
                self.mouse = !self.mouse;
                self.config.mouse = self.mouse;
                self.status = format!(
                    "mouse {} — {}",
                    if self.mouse { "on" } else { "off" },
                    if self.mouse { "wheel scrolls, click selects" } else { "terminal text selection works again" }
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

    // ---- cursor movement --------------------------------------------------

    fn len_of(&self, pane: Pane) -> usize {
        match pane {
            Pane::Messages => self.visible_messages().len(),
            Pane::Signals => self.current_message().map_or(0, |m| m.signals.len()),
            Pane::Rx => self.rx.len(),
            Pane::Cyclic => self.cyclic.len(),
            Pane::Rules => self.rules.rules.len(),
        }
    }

    fn scroll_of(&self, pane: Pane) -> usize {
        self.scroll_ref(pane).offset
    }

    fn scroll_ref(&self, pane: Pane) -> &Scroll {
        match pane {
            Pane::Messages => &self.msg_scroll,
            Pane::Signals => &self.sig_scroll,
            Pane::Rx => &self.rx_scroll,
            Pane::Cyclic => &self.cyclic_scroll,
            Pane::Rules => &self.rules_scroll,
        }
    }

    fn scroll_mut(&mut self, pane: Pane) -> &mut Scroll {
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
    fn page_size(&self, pane: Pane) -> usize {
        let height = self.layout.rect_of(pane).height;
        (height.saturating_sub(pane.header_rows() + 1) as usize).max(1)
    }

    fn move_cursor(&mut self, step: i32) {
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

    fn set_cursor(&mut self, index: usize) {
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
                self.sig_scroll = Scroll { offset: 0, follow: true };
            }
            Pane::Signals => self.sig_index = index,
            Pane::Rx => self.rx_index = index,
            Pane::Cyclic => self.cyclic_index = index,
            Pane::Rules => self.rules_index = index,
        }
    }

    // ---- sending ----------------------------------------------------------

    fn send_once(&mut self) {
        if self.link != Link::Up {
            self.status = "not connected — press F4 to pick an interface".into();
            return;
        }
        if let Some((name, id, data)) = self.encode_current() {
            self.send_command(Command::Send { id, data });
            self.status = format!("sent {name}  {}", data.hex());
        }
    }

    fn toggle_cyclic(&mut self) {
        let Some(msg) = self.current_message().cloned() else { return };
        if self.cyclic.contains_key(&msg.name) {
            self.send_command(Command::ClearCyclic(msg.name.clone()));
            self.cyclic.remove(&msg.name);
            self.status = format!("stopped {}", msg.name);
            return;
        }
        if self.link == Link::Down {
            self.status = "not connected — press F4 to pick an interface".into();
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

    // ---- rules ------------------------------------------------------------

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

    fn reload_rules(&mut self) {
        match self.rules.path.clone() {
            Some(path) => self.load_rules(&path),
            None => {
                self.status = "no rules file loaded — start tuican with --rules FILE".into()
            }
        }
    }

    fn toggle_selected_rule(&mut self) {
        let Some(rule) = self.rules.rules.get_mut(self.rules_index) else { return };
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

    fn apply_act(&mut self, rule: &str, act: &Act) {
        match act {
            Act::Set { message, signal, value } => {
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

    /// Push new bytes into a periodic send that is already running.
    fn rearm_cyclic(&mut self, message: &str) {
        let Some(period) = self.cyclic.get(message).map(|e| e.period) else { return };
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

    fn stop_selected_cyclic(&mut self) {
        let Some(name) = self.selected_cyclic() else { return };
        self.send_command(Command::ClearCyclic(name.clone()));
        self.cyclic.remove(&name);
        self.cyclic_index = self.cyclic_index.min(self.cyclic.len().saturating_sub(1));
        self.status = format!("stopped {name}");
    }

    /// Put the message cursor on the selected periodic send, clearing any
    /// filter that would otherwise hide it.
    fn jump_to_selected_cyclic(&mut self) {
        let Some(name) = self.selected_cyclic() else { return };
        if !self.visible_messages().iter().any(|m| m.name == name) {
            self.filter.clear();
        }
        if let Some(i) = self.visible_messages().iter().position(|m| m.name == name) {
            self.pane = Pane::Messages;
            self.set_cursor(i);
            self.status = format!("jumped to {name}");
        }
    }

    fn begin_edit_signal(&mut self) {
        let Some(msg) = self.current_message().cloned() else { return };
        let Some(sig) = msg.signals.get(self.sig_index.min(msg.signals.len().saturating_sub(1)))
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
        let current = self.values_of(&msg.name).get(&sig.name).copied().unwrap_or(0.0);
        self.prompt_with_default(
            PromptKind::SignalValue,
            format!("{}{hint} =", sig.name),
            trim(current),
        );
    }

    // ---- prompts ----------------------------------------------------------

    fn prompt(&mut self, kind: PromptKind, label: String, buffer: String) {
        self.mode = Mode::Prompt(Prompt { kind, label, buffer, default: None });
    }

    fn prompt_with_default(&mut self, kind: PromptKind, label: String, default: String) {
        self.mode = Mode::Prompt(Prompt {
            kind,
            label,
            buffer: String::new(),
            default: Some(default),
        });
    }

    fn update_prompt(&mut self, action: Action) {
        let Mode::Prompt(prompt) = &mut self.mode else { return };
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

    fn apply_filter(&mut self) {
        let Mode::Prompt(p) = &self.mode else { return };
        self.filter = p.buffer.clone();
        self.msg_index = 0;
        self.msg_scroll = Scroll { offset: 0, follow: true };
    }

    fn submit_prompt(&mut self) {
        let Mode::Prompt(prompt) = &self.mode else { return };
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

    fn commit_signal_value(&mut self, text: &str) {
        let Some(msg) = self.current_message().cloned() else { return };
        let Some(sig) = msg.signals.get(self.sig_index.min(msg.signals.len().saturating_sub(1)))
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
            self.cyclic.insert(name.clone(), CyclicEntry { id, period, data });
            self.send_command(Command::SetCyclic { key: name, id, data, period });
        }
    }

    fn commit_period(&mut self, text: &str) {
        let Ok(ms) = text.parse::<f64>() else {
            self.status = format!("{text:?} is not a period in milliseconds");
            return;
        };
        if ms <= 0.0 {
            self.status = "period must be greater than zero".into();
            return;
        }
        let Some((name, id, data)) = self.encode_current() else { return };
        let period = Duration::from_secs_f64(ms / 1000.0);
        self.send_command(Command::SetCyclic { key: name.clone(), id, data, period });
        self.cyclic.insert(name.clone(), CyclicEntry { id, period, data });
        self.status = format!("sending {name} every {} ms", trim(ms));
    }

    fn commit_raw_send(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let parts: Vec<&str> = text
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        let Some((id_text, rest)) = parts.split_first() else { return };
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

    // ---- pickers ----------------------------------------------------------

    pub fn open_interface_picker(&mut self, startup: bool) {
        let found = discover::scan(self.config.bitrate);
        let last = self.config.interface.clone();
        let index = last
            .as_ref()
            .and_then(|l| found.iter().position(|c| c.spec.same_device(l)))
            .unwrap_or(0);
        self.mode = Mode::Picker(Picker {
            title: "interface".into(),
            rows: found
                .iter()
                .map(|c| PickerRow { label: c.label.clone(), detail: c.detail.clone() })
                .collect(),
            index,
            kind: PickerKind::Interface(found.into_iter().map(|c| c.spec).collect()),
            startup,
            then_interface: false,
        });
    }

    pub fn open_dbc_picker(&mut self, startup: bool) {
        self.open_dbc_picker_chaining(startup, false);
    }

    /// `then_interface` chains straight into the interface picker on accept.
    pub fn open_dbc_picker_chaining(&mut self, startup: bool, then_interface: bool) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let found = dbc::load::discover(&cwd);
        if found.is_empty() {
            self.status = "no .dbc files here — pass one with --dbc".into();
            if !startup {
                self.mode = Mode::Normal;
            }
            return;
        }
        let index = self
            .config
            .dbc
            .as_ref()
            .and_then(|d| found.iter().position(|p| p == d))
            .unwrap_or(0);
        self.mode = Mode::Picker(Picker {
            title: "dbc file".into(),
            rows: found
                .iter()
                .map(|p| PickerRow {
                    label: p.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                    // Relative to where we were launched: `dbc/primary` reads
                    // far better than a home-directory-deep absolute path.
                    detail: p
                        .parent()
                        .map(|d| d.strip_prefix(&cwd).unwrap_or(d).display().to_string())
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| ".".into()),
                })
                .collect(),
            index,
            kind: PickerKind::Dbc(found),
            startup,
            then_interface,
        });
    }

    fn open_bitrate_picker(&mut self, spec: TransportSpec, startup: bool) {
        let index = COMMON_BITRATES
            .iter()
            .position(|&b| b == self.config.bitrate)
            .unwrap_or(0);
        self.mode = Mode::Picker(Picker {
            title: "bitrate".into(),
            rows: COMMON_BITRATES
                .iter()
                .map(|&b| PickerRow { label: crate::fmt_bitrate(b), detail: String::new() })
                .collect(),
            index,
            kind: PickerKind::Bitrate(spec),
            startup,
            then_interface: false,
        });
    }

    fn update_picker(&mut self, action: Action) {
        let Mode::Picker(picker) = &mut self.mode else { return };
        match action {
            Action::PickerMove(step) | Action::Move(step) => {
                let len = picker.rows.len().max(1);
                picker.index =
                    (picker.index as i32 + step).rem_euclid(len as i32) as usize;
            }
            Action::Cancel => {
                let startup = picker.startup;
                self.mode = Mode::Normal;
                if startup {
                    self.status =
                        "skipped — F4 picks an interface, F3 loads a DBC, ? shows the keys".into();
                }
            }
            Action::PickerAccept | Action::PromptSubmit => self.accept_picker(),
            Action::Quit => self.quit(),
            _ => {}
        }
    }

    fn accept_picker(&mut self) {
        let Mode::Picker(picker) = &mut self.mode else { return };
        let index = picker.index;
        let startup = picker.startup;
        let then_interface = picker.then_interface;
        let kind = std::mem::replace(&mut picker.kind, PickerKind::Interface(Vec::new()));
        self.mode = Mode::Normal;

        match kind {
            PickerKind::Interface(specs) => {
                let Some(spec) = specs.into_iter().nth(index) else { return };
                if spec.needs_bitrate() {
                    self.open_bitrate_picker(spec, startup);
                } else {
                    self.connect(spec, startup);
                }
            }
            PickerKind::Bitrate(spec) => {
                let rate = COMMON_BITRATES.get(index).copied().unwrap_or(500_000);
                self.config.bitrate = rate;
                self.connect(spec.with_bitrate(rate), startup);
            }
            PickerKind::Dbc(paths) => {
                if let Some(path) = paths.into_iter().nth(index) {
                    self.load_dbc(&path);
                }
                // On launch the DBC comes first, and the interface follows —
                // unless --interface already settled it.
                if then_interface {
                    self.open_interface_picker(true);
                }
            }
        }
    }

    /// Connect without going through the picker (CLI flags, or `--last`).
    pub fn connect_to(&mut self, spec: TransportSpec) {
        self.connect(spec, false);
    }

    fn connect(&mut self, spec: TransportSpec, _startup: bool) {
        self.status = "connecting…".into();
        self.send_command(Command::Connect(spec));
    }

    // ---- the DBC ----------------------------------------------------------

    /// Swap the loaded file. On failure the old database stays live, so a typo
    /// cannot leave you with nothing loaded.
    pub fn load_dbc(&mut self, path: &std::path::Path) {
        match dbc::load::load(path) {
            Ok(db) => {
                let count = db.messages.len();
                self.db = Arc::new(db);
                self.values.clear(); // signal edits belonged to the old file
                self.rx.redecode(&self.db); // keep the counters, redo the names
                self.msg_index = 0;
                self.sig_index = 0;
                self.msg_scroll = Scroll { offset: 0, follow: true };
                self.sig_scroll = Scroll { offset: 0, follow: true };
                self.config.remember_dbc(path);
                self.status = format!("loaded {} — {count} messages", self.db.label());
                // Rule names were validated against the old database.
                if let Some(rules) = self.rules.path.clone() {
                    self.load_rules(&rules);
                    self.status = format!("loaded {} — {count} messages", self.db.label());
                }
            }
            Err(e) => {
                self.status = format!("{e:#}");
                self.last_error = Some(format!("{e:#}"));
            }
        }
    }

    pub fn now(&self) -> Instant {
        Instant::now()
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

    fn next_pane(&self, step: i32) -> Pane {
        let panes = self.panes();
        let i = panes.iter().position(|p| *p == self.pane).unwrap_or(0) as i32;
        panes[(i + step).rem_euclid(panes.len() as i32) as usize]
    }

    /// Name of the periodic send under the cursor in the panel.
    pub fn selected_cyclic(&self) -> Option<String> {
        self.cyclic.keys().nth(self.cyclic_index).cloned()
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
    use crate::rules::{Condition, Op, Rule, Source};

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
            then: vec![Act::Set { message: "M".into(), signal: "a".into(), value: to }],
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
        app.values.insert("M".into(), HashMap::from([("a".to_string(), 0.0)]));

        app.run_rules();

        assert!(app.rules.looped, "a cycle should be reported");
        assert!(
            app.last_error.as_deref().is_some_and(|e| e.contains("loop")),
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
        app.values.insert("M".into(), HashMap::from([("a".to_string(), 0.0)]));

        app.run_rules();

        assert!(!app.rules.looped);
        assert_eq!(app.rules.rules[0].fired, 1);
        assert_eq!(app.values["M"]["a"], 1.0);
    }
}
