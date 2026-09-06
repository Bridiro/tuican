//! What the user *meant*, decoupled from which key or click produced it.
//!
//! Both `ui::input` and `ui::mouse` produce these, which is what keeps mouse
//! support from growing its own parallel set of behaviours.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Messages,
    Signals,
    Rx,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::Messages, Pane::Signals, Pane::Rx];

    pub fn name(self) -> &'static str {
        match self {
            Pane::Messages => "messages",
            Pane::Signals => "signals",
            Pane::Rx => "receive",
        }
    }

    /// Rows between the pane's top edge and its first item: border, plus a
    /// column header where the pane has one.
    pub fn header_rows(self) -> u16 {
        match self {
            Pane::Rx => 2,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Quit,

    NextPane,
    PrevPane,
    Move(i32),
    Page(i32),
    Home,
    End,
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

    PromptChar(char),
    PromptBackspace,
    PromptSubmit,
    Cancel,

    PickerMove(i32),
    PickerAccept,
}
