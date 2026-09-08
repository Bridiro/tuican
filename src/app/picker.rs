//! The interface, bitrate and DBC choosers.

use super::action::Action;
use super::{App, Mode, Picker, PickerKind, PickerRow};
use crate::bus::command::Command;
use crate::dbc;
use crate::transport::discover;
use crate::transport::spec::{COMMON_BITRATES, TransportSpec};

impl App {
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
                .map(|c| PickerRow {
                    label: c.label.clone(),
                    detail: c.detail.clone(),
                })
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
            self.status = "no .dbc files here, pass one with --dbc".into();
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
                    label: p
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
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

    pub(super) fn open_bitrate_picker(&mut self, spec: TransportSpec, startup: bool) {
        let index = COMMON_BITRATES
            .iter()
            .position(|&b| b == self.config.bitrate)
            .unwrap_or(0);
        self.mode = Mode::Picker(Picker {
            title: "bitrate".into(),
            rows: COMMON_BITRATES
                .iter()
                .map(|&b| PickerRow {
                    label: crate::fmt_bitrate(b),
                    detail: String::new(),
                })
                .collect(),
            index,
            kind: PickerKind::Bitrate(spec),
            startup,
            then_interface: false,
        });
    }

    pub(super) fn update_picker(&mut self, action: Action) {
        let Mode::Picker(picker) = &mut self.mode else {
            return;
        };
        match action {
            Action::PickerMove(step) | Action::Move(step) => {
                let len = picker.rows.len().max(1);
                picker.index = (picker.index as i32 + step).rem_euclid(len as i32) as usize;
            }
            Action::Cancel => {
                let startup = picker.startup;
                self.mode = Mode::Normal;
                if startup {
                    self.status =
                        "skipped. F4 picks an interface, F3 loads a DBC, ? shows the keys".into();
                }
            }
            Action::PickerAccept | Action::PromptSubmit => self.accept_picker(),
            Action::Quit => self.quit(),
            _ => {}
        }
    }

    pub(super) fn accept_picker(&mut self) {
        let Mode::Picker(picker) = &mut self.mode else {
            return;
        };
        let index = picker.index;
        let startup = picker.startup;
        let then_interface = picker.then_interface;
        let kind = std::mem::replace(&mut picker.kind, PickerKind::Interface(Vec::new()));
        self.mode = Mode::Normal;

        match kind {
            PickerKind::Interface(specs) => {
                let Some(spec) = specs.into_iter().nth(index) else {
                    return;
                };
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
                // On launch the DBC comes first, and the interface follows,
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

    pub(super) fn connect(&mut self, spec: TransportSpec, _startup: bool) {
        self.status = "connecting…".into();
        self.send_command(Command::Connect(spec));
    }
}
