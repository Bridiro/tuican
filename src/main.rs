//! tuican — a terminal CAN bench.
//!
//! Three actors, no shared mutable state: this thread owns the UI and every
//! piece of state, one background thread owns the adapter, and they exchange
//! `Command` and `Event` values over channels. See DESIGN.md.

mod app;
mod bus;
mod canid;
mod cli;
mod config;
mod dbc;
mod rules;
mod transport;
mod ui;

use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};

use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyEventKind, poll, read,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;
use config::Config;
use transport::TransportSpec;

/// `1 Mbit/s`, `500 kbit/s` — used in the header and every picker row.
pub fn fmt_bitrate(bps: u32) -> String {
    if bps.is_multiple_of(1_000_000) {
        format!("{} Mbit/s", bps / 1_000_000)
    } else if bps.is_multiple_of(1_000) {
        format!("{} kbit/s", bps / 1_000)
    } else {
        format!("{bps} bit/s")
    }
}

/// ~30 fps. A busy bus produces far more events than frames; they are coalesced
/// into whatever the next draw shows.
const FRAME: Duration = Duration::from_millis(33);

fn main() -> anyhow::Result<()> {
    let args = cli::Cli::parse();
    if args.print_udev_rule {
        print!("{}", transport::discover::UDEV_RULE);
        return Ok(());
    }
    let _log_guard = init_logging();

    let mut config = Config::load();
    if let Some(b) = args.bitrate {
        config.bitrate = b;
    }

    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    let (evt_tx, evt_rx) = crossbeam_channel::unbounded();
    let bus = bus::spawn(cmd_rx, evt_tx);

    let mut app = App::new(cmd_tx, config);
    let startup = plan_startup(&mut app, &args);

    let mut terminal = setup(app.mouse)?;
    let result = run(&mut terminal, &mut app, &evt_rx, startup);
    restore()?;

    app.config.mouse = app.mouse;
    app.config.save();
    let _ = bus.join();

    if let Err(e) = &result {
        eprintln!("tuican: {e:#}");
    }
    result
}

/// What the user still has to choose on launch. Each flag is independent, so
/// `--dbc` alone skips only the DBC picker.
struct Startup {
    need_dbc: bool,
    need_interface: bool,
}

fn plan_startup(app: &mut App, args: &cli::Cli) -> Startup {
    // 1. A DBC, from the flag, from --last, or not at all.
    let dbc = args
        .dbc
        .clone()
        .or_else(|| args.last.then(|| app.config.dbc.clone()).flatten());
    if let Some(path) = dbc {
        app.load_dbc(&path);
    }

    // 2. Rules, once the DBC is in place: loading them second is what lets the
    //    loader check every message and signal name against it.
    let rules = args
        .rules
        .clone()
        .or_else(|| args.last.then(|| app.config.rules.clone()).flatten())
        .filter(|p| p.is_file())
        .or_else(|| rules::load::discover(app.config.dbc.as_deref()));
    if let Some(path) = rules {
        app.load_rules(&path);
    }

    // 3. An interface, if the arguments fully describe one.
    let spec = spec_from_args(args).or_else(|| {
        args.last
            .then(|| app.config.interface.clone())
            .flatten()
            .map(|s| s.with_bitrate(app.config.bitrate))
    });
    let need_interface = match spec {
        Some(spec) => {
            app.connect_to(spec);
            false
        }
        None => true,
    };

    Startup { need_dbc: app.db.is_empty(), need_interface }
}

fn spec_from_args(args: &cli::Cli) -> Option<TransportSpec> {
    let bitrate = args.bitrate.unwrap_or(500_000);
    match args.interface.as_deref()? {
        "virtual" | "vcan" => Some(TransportSpec::Virtual),
        "slcan" => args
            .channel
            .clone()
            .map(|port| TransportSpec::Slcan { port, bitrate }),
        #[cfg(target_os = "linux")]
        "socketcan" => args
            .channel
            .clone()
            .map(|iface| TransportSpec::SocketCan { iface }),
        // gs_usb has no stable channel name, so we still have to look for it.
        "gs_usb" | "candlelight" => transport::discover::scan(bitrate)
            .into_iter()
            .map(|c| c.spec)
            .find(|s| matches!(s, TransportSpec::GsUsb { .. })),
        _ => None,
    }
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    events: &crossbeam_channel::Receiver<bus::event::Event>,
    startup: Startup,
) -> anyhow::Result<()> {
    // The DBC comes first: choosing an interface is more meaningful once the
    // message list is populated.
    if startup.need_dbc {
        app.open_dbc_picker_chaining(true, startup.need_interface);
    } else if startup.need_interface {
        app.open_interface_picker(true);
    }

    let mut mouse_on = app.mouse;
    let mut next = Instant::now();
    loop {
        // Coalesce: many frames may have arrived since the last draw.
        while let Ok(event) = events.try_recv() {
            app.on_bus_event(event);
        }
        // Rules see the batch as one state, so a rule that watches two messages
        // does not fire on a half-updated picture.
        app.run_rules();

        while poll(Duration::ZERO)? {
            match read()? {
                CEvent::Key(k) if k.kind == KeyEventKind::Press => {
                    let action = ui::input::map(k, &app.mode, app.pending);
                    app.update(action);
                }
                CEvent::Mouse(m) if app.mouse => {
                    let action = ui::mouse::map(m, &app.layout, &app.mode);
                    app.update(action);
                }
                // Nothing to do: the next draw lays out from the new size.
                CEvent::Resize(..) => {}
                _ => {}
            }
        }

        if app.should_quit {
            return Ok(());
        }
        if app.mouse != mouse_on {
            mouse_on = app.mouse;
            if mouse_on {
                execute!(stdout(), EnableMouseCapture)?;
            } else {
                // Releasing capture hands the terminal's own text selection
                // back to the user, which is why this is a toggle at all.
                execute!(stdout(), DisableMouseCapture)?;
            }
        }
        terminal.draw(|f| ui::draw(f, app))?;

        next += FRAME;
        let now = Instant::now();
        if next < now {
            next = now; // we fell behind; do not spin trying to catch up
        }
        std::thread::sleep(next.saturating_duration_since(now));
    }
}

fn setup(mouse: bool) -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    // Without this, a panic leaves the user in a raw-mode alternate screen with
    // no echo and no obvious way back.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        previous(info);
    }));

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    if mouse {
        execute!(stdout(), EnableMouseCapture)?;
    }
    Ok(Terminal::new(CrosstermBackend::new(stdout()))?)
}

fn restore() -> anyhow::Result<()> {
    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}

/// stdout is the TUI, so everything goes to a file. `RUST_LOG=debug tuican`.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dir = directories::ProjectDirs::from("", "", "tuican")?
        .state_dir()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&dir).ok()?;
    let file = tracing_appender::rolling::never(&dir, "tuican.log");
    let (writer, guard) = tracing_appender::non_blocking(file);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "tuican=info".into()),
        )
        .init();
    Some(guard)
}
