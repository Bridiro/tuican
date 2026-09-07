//! What the user *meant*, decoupled from which key or click produced it.
//!
//! Both `ui::input` and `ui::mouse` produce these, which is what keeps mouse
//! support from growing its own parallel set of behaviours.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Messages,
    Signals,
    Rx,
    /// The periodic-send list. Only reachable while its panel is open.
    Cyclic,
    /// The reaction rules. Only reachable while its panel is open.
    Rules,
}

impl Pane {
    pub const ALL: [Pane; 5] =
        [Pane::Messages, Pane::Signals, Pane::Rx, Pane::Cyclic, Pane::Rules];

    pub fn name(self) -> &'static str {
        match self {
            Pane::Messages => "messages",
            Pane::Signals => "signals",
            Pane::Rx => "receive",
            Pane::Cyclic => "cyclic",
            Pane::Rules => "rules",
        }
    }

    /// Rows between the pane's top edge and its first item: border, plus a
    /// column header where the pane has one.
    pub fn header_rows(self) -> u16 {
        match self {
            Pane::Rx | Pane::Cyclic | Pane::Rules => 2,
            _ => 1,
        }
    }
}

/// Where `H`, `M` and `L` land within the visible window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSpot {
    Top,
    Middle,
    Bottom,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Quit,

    NextPane,
    PrevPane,
    Move(i32),
    Page(i32),
    /// Ctrl-D / Ctrl-U.
    HalfPage(i32),
    Home,
    /// Bottom, or with a count prefix, that row.
    End,
    /// H / M / L: top, middle, bottom of what is currently on screen.
    Window(WindowSpot),
    /// A digit typed before a motion, e.g. the `5` of `5j`.
    CountDigit(u32),
    /// The first `g` of `gg`.
    BeginG,
    /// Click: focus a pane and put the cursor on a row, in one step.
    FocusAndSelect(Pane, usize),
    /// Wheel: scroll the pane under the cursor without moving focus.
    ScrollIn(Option<Pane>, i32),

    BeginFilter,
    BeginEditSignal,
    SendOnce,
    ToggleCyclic,
    StopAllCyclic,
    BeginRawSend,
    ClearRx,

    OpenInterfacePicker,
    OpenDbcPicker,
    ToggleMouse,
    ToggleHelp,
    ToggleCyclicPanel,
    /// Stop just the periodic send under the cursor.
    StopSelectedCyclic,
    ToggleRulesPanel,
    ReloadRules,

    PromptChar(char),
    PromptBackspace,
    PromptSubmit,
    Cancel,

    PickerMove(i32),
    PickerAccept,
}
