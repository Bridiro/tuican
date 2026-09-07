//! Arguments. Every one is optional: with none, the tool discovers what is
//! plugged in and asks. That is the whole point of the rewrite.

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "tuican",
    about = "A terminal CAN bench: pick an adapter, load a DBC, send and watch traffic.",
    version
)]
pub struct Cli {
    /// DBC file to load. Without it, tuican offers the ones it finds here.
    #[arg(long, value_name = "FILE")]
    pub dbc: Option<PathBuf>,

    /// Reaction rules to load. Without it, tuican looks for
    /// `tuican.rules.toml` beside the DBC and in the working directory.
    #[arg(long, value_name = "FILE")]
    pub rules: Option<PathBuf>,

    /// Adapter to use: gs_usb, socketcan, slcan, or virtual.
    #[arg(long, value_name = "KIND")]
    pub interface: Option<String>,

    /// Channel for the chosen adapter: `can0`, or a serial port for slcan.
    #[arg(long, value_name = "NAME")]
    pub channel: Option<String>,

    #[arg(long, value_name = "BPS")]
    pub bitrate: Option<u32>,

    /// Reconnect to the last session and skip both pickers.
    #[arg(long)]
    pub last: bool,

    /// Print the udev rule that fixes permission errors on Linux, then exit.
    #[arg(long)]
    pub print_udev_rule: bool,
}
