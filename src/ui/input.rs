//! Keys → [`Action`]. The only file that knows which key does what.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::action::Action;
use crate::app::Mode;

pub fn map(key: KeyEvent, mode: &Mode) -> Action {
    match mode {
        // While a prompt is open the keyboard belongs to it: typing `q` must
        // type a `q`, not quit.
        Mode::Prompt(_) => match key.code {
            KeyCode::Esc => Action::Cancel,
            KeyCode::Enter => Action::PromptSubmit,
            KeyCode::Backspace => Action::PromptBackspace,
            KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                Action::Cancel
            }
            KeyCode::Char(c) => Action::PromptChar(c),
            _ => Action::None,
        },
        Mode::Picker(_) => match key.code {
            KeyCode::Esc => Action::Cancel,
            KeyCode::Enter => Action::PickerAccept,
            KeyCode::Up | KeyCode::Char('k') => Action::PickerMove(-1),
            KeyCode::Down | KeyCode::Char('j') => Action::PickerMove(1),
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            _ => Action::None,
        },
        Mode::Help => Action::Cancel,
        Mode::Normal => normal(key),
    }
}

fn normal(key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            _ => Action::None,
        };
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Tab => Action::NextPane,
        KeyCode::BackTab => Action::PrevPane,
        KeyCode::Up | KeyCode::Char('k') => Action::Move(-1),
        KeyCode::Down | KeyCode::Char('j') => Action::Move(1),
        KeyCode::PageUp => Action::Page(-1),
        KeyCode::PageDown => Action::Page(1),
        KeyCode::Home | KeyCode::Char('g') => Action::Home,
        KeyCode::End | KeyCode::Char('G') => Action::End,
        KeyCode::Char('/') => Action::BeginFilter,
        KeyCode::Enter => Action::BeginEditSignal,
        KeyCode::Char('s') => Action::SendOnce,
        KeyCode::Char('p') => Action::ToggleCyclic,
        KeyCode::Char('x') => Action::StopAllCyclic,
        KeyCode::Char('r') => Action::BeginRawSend,
        KeyCode::Char('c') => Action::ClearRx,
        KeyCode::F(2) => Action::ToggleMouse,
        KeyCode::F(3) => Action::OpenDbcPicker,
        KeyCode::F(4) => Action::OpenInterfacePicker,
        KeyCode::Char('?') => Action::ToggleHelp,
        _ => Action::None,
    }
}
