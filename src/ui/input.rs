//! Keys → [`Action`]. The only file that knows which key does what.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::action::{Action, WindowSpot};
use crate::app::{Mode, Pending};

pub fn map(key: KeyEvent, mode: &Mode, pending: Pending) -> Action {
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
        Mode::Normal => normal(key, pending),
    }
}

fn normal(key: KeyEvent, pending: Pending) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            KeyCode::Char('d') => Action::HalfPage(1),
            KeyCode::Char('u') => Action::HalfPage(-1),
            KeyCode::Char('f') => Action::Page(1),
            KeyCode::Char('b') => Action::Page(-1),
            _ => Action::None,
        };
    }
    // Waiting on the second key of `gg`. Anything else abandons the sequence,
    // which is also how the caller learns to clear the pending state.
    if pending == Pending::G {
        return match key.code {
            KeyCode::Char('g') => Action::Home,
            _ => Action::None,
        };
    }
    // Count prefixes come first so `5` is never read as a command. A leading
    // zero is dropped by the reducer, leaving `0` free of meaning here.
    if let KeyCode::Char(c) = key.code
        && let Some(d) = c.to_digit(10)
    {
        return Action::CountDigit(d);
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Tab => Action::NextPane,
        KeyCode::BackTab => Action::PrevPane,
        KeyCode::Up | KeyCode::Char('k') => Action::Move(-1),
        KeyCode::Down | KeyCode::Char('j') => Action::Move(1),
        KeyCode::PageUp => Action::Page(-1),
        KeyCode::PageDown => Action::Page(1),
        KeyCode::Home => Action::Home,
        KeyCode::Char('g') => Action::BeginG,
        KeyCode::End | KeyCode::Char('G') => Action::End,
        KeyCode::Char('H') => Action::Window(WindowSpot::Top),
        KeyCode::Char('M') => Action::Window(WindowSpot::Middle),
        KeyCode::Char('L') => Action::Window(WindowSpot::Bottom),
        // No horizontal cursor to move, so h/l take the pane-switch role.
        KeyCode::Char('l') => Action::NextPane,
        KeyCode::Char('h') => Action::PrevPane,
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
        KeyCode::F(5) => Action::ToggleCyclicPanel,
        KeyCode::F(6) => Action::ToggleRulesPanel,
        KeyCode::F(7) => Action::ReloadRules,
        KeyCode::Char('d') => Action::StopSelectedCyclic,
        KeyCode::Char('?') => Action::ToggleHelp,
        _ => Action::None,
    }
}
