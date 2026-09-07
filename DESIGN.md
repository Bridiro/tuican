# tuican — design & implementation plan

A terminal CAN bench in Rust: pick an adapter, load a DBC, inspect and craft frames.
Feature parity with `can-bench.py`, but with discovery instead of flags, runtime
interface switching, and a layout that survives a resize.

**Status: built.** The code in `src/` implements this document. Where the two
differ, the code is right and §10 records what changed and why. `README.md` is
the user-facing half.

---

## 1. Architecture

Three long-lived actors. No shared mutable state between them — only channels.

```
                       ┌──────────────────────────────────────────────┐
                       │              main / UI thread                │
                       │                                              │
  terminal events ───► │  input::map ──► Action ──► App::update       │
  (crossterm reader)   │                              │               │
                       │                              ▼               │
                       │                        App (single owner of  │
                       │                        all UI + RX state)    │
                       │                              │               │
                       │                        ui::draw(frame, &App) │
                       │                              │               │
                       └──────────────┬───────────────┴───────────────┘
                                      │  Command                ▲ Event
                                      ▼  (crossbeam)            │ (crossbeam)
                       ┌──────────────────────────────────────────────┐
                       │                  bus thread                  │
                       │                                              │
                       │  select {                                    │
                       │    cmd  => connect / disconnect / send /      │
                       │            set_cyclic / clear_cyclic         │
                       │    tick => fire due cyclic frames            │
                       │    else => transport.recv(deadline)          │
                       │  }                                           │
                       │                                              │
                       │  Box<dyn Transport>   ◄── exactly one, ever   │
                       └──────────────┬───────────────────────────────┘
                                      │
              ┌───────────────────────┼───────────────────────┐
              ▼                       ▼                       ▼
       SocketCanTransport      GsUsbTransport          SlcanTransport
       (linux only)            (rusb / candleLight)    (serialport)

    ┌──────────────────────────────────────────────────────────────┐
    │ dbc:: — pure, no I/O, shared by value (Arc<Database>)         │
    │  load.rs  can-dbc ──► our own Database model                  │
    │  codec.rs bit extract/insert, scale/offset, sign, mux         │
    └──────────────────────────────────────────────────────────────┘
```

**Data flow, TX:** user edits signal values in `App` → `codec::encode` (UI thread,
pure, instant feedback on failure) → `Command::Send{ id, data, ext }` → bus thread
writes to the transport. The bus thread never sees the DBC.

**Data flow, RX:** transport yields a `Frame` → `Event::Rx(Frame, Instant)` → UI
thread decodes it against the currently loaded DBC and folds it into its RX table.

Two consequences worth stating, because they're what make the whole thing simple:

1. **Decoding lives on the UI side.** Swapping the DBC at runtime is then just
   `app.db = new_db` plus clearing cached decode strings. The bus thread doesn't
   restart, the connection doesn't drop, in-flight cyclic sends keep their bytes.
2. **The "only one interface" constraint is enforced by ownership.** The bus thread
   holds one `Option<Box<dyn Transport>>`. Connecting drops the old one first. There
   is no code path that can hold two.

**Why not `tokio`.** The three real transports are blocking: `rusb` bulk transfers,
`SocketCAN` read, serial read. Async buys nothing here and costs a runtime, `Send +
'static` bounds on transport objects, and an awkward story for the 20 ms RX drain
loop. One OS thread with `crossbeam_channel::select!` expresses the scheduler
(commands, cyclic deadlines, RX drain) more directly than `tokio::select!` would.

---

## 2. Crates

| Crate | Why |
|---|---|
| `ratatui` | The maintained successor to `tui-rs`. Immediate-mode: every frame is laid out from the *current* terminal size, so resize is handled by construction rather than by patching. |
| `crossterm` | ratatui's cross-platform backend; also the event source (`Key`, `Resize`, `Mouse`) and the raw-mode / alternate-screen / mouse-capture switches. Works on macOS, which `termion` does not. |
| `can-dbc` | Pure-Rust parser for DBC files (v10 is a `pest` rewrite exposing plain public fields, `Dbc::try_from(&str)`). **Parsing only** — it has no encode or decode. `dbc::codec` is ours (§5), and it is the highest-risk module in the project. |
| `rusb` | libusb bindings. This is the gs_usb / candleLight path, and the only one that works on macOS (there is no macOS kernel driver for candleLight; libusb claims the interface directly). |
| `socketcan` | Native `can0` on Linux. `#[cfg(target_os = "linux")]`-gated — it does not build elsewhere. |
| `serialport` | slcan / Lawicel adapters (CANable in slcan firmware, USBtin, …). Cheap breadth. |
| `crossbeam-channel` | `select!` with timeouts across the command channel and the cyclic deadline. `std::sync::mpsc` cannot do multi-source select with a deadline. |
| `clap` (derive) | Optional overrides — `--dbc`, `--interface`, `--bitrate`. Every one has a discovery fallback, so the tool runs with zero flags. |
| `anyhow` + `thiserror` | `thiserror` for `TransportError` / `CodecError` (the UI matches on them); `anyhow` at the edges. |
| `serde` + `toml` + `directories` | Persist last interface, last DBC path, per-message signal values. Second launch is one keypress. |
| `tracing` + `tracing-appender` | **stdout is the TUI.** All logging goes to a rolling file (`~/.local/state/tuican/tuican.log`); `RUST_LOG=debug` there, never on screen. |
| `embedded-can` | `Id` / `StandardId` / `ExtendedId` newtypes, so extended-vs-standard is a type distinction rather than a `bool` argument that gets passed wrong. |

Deliberately not used: `tokio` (§1), `dbc-codegen` (generates Rust from a DBC at
*build* time — incompatible with loading a DBC at runtime), `cursive`/`tui-realm`
(retained-mode; resize becomes something you handle instead of something free).

---

## 3. Modules

```
src/
  main.rs             terminal setup/teardown (incl. panic hook), event loop
  cli.rs              clap args; all optional
  config.rs           load/save ~/.config/tuican/config.toml

  bus/
    mod.rs            spawn(); the select! scheduler
    command.rs        Command enum  (UI → bus)
    event.rs          Event enum    (bus → UI)
    cyclic.rs         periodic TX table, next-deadline computation

  transport/
    mod.rs            trait Transport, TransportError, Frame
    spec.rs           TransportSpec (serialisable "how to connect"), open()
    discover.rs       enumerate USB VID/PID, /sys/class/net/can*, serial ports
    gs_usb.rs         candleLight/gs_usb over rusb: control setup + bulk I/O
    socketcan.rs      #[cfg(target_os = "linux")]
    slcan.rs          ASCII protocol over serialport
    virt.rs           loopback + replay; makes the UI testable with no hardware

  dbc/
    mod.rs            Database, MessageDef, SignalDef  (our model, not can-dbc's)
    load.rs           can-dbc → our model; the only file that knows can-dbc
    codec.rs          encode / decode: bit layout, scale, offset, sign
    mux.rs            multiplexor resolution, default values, representable range

  app/
    mod.rs            App: all state, `update(Action)`, `on_event(Event)`
    action.rs         Action enum (what the user meant, not which key they hit)
    rx_table.rs       per-ID row: count, smoothed period, last data, decoded text
    editor.rs         the modal signal-value / raw-frame / filter prompt

  ui/
    mod.rs            draw(frame, &App) — the single layout entry point
    layout.rs         responsive constraint sets + the LayoutMap for hit-testing
    panes/            messages.rs, signals.rs, rx.rs, status.rs, picker.rs
    mouse.rs          Rect hit-testing → Action
    input.rs          KeyEvent → Action
    theme.rs          styles in one place
```

Responsibilities in one line each:

- **`transport`** — turns bytes on a wire into `Frame`s. Knows nothing about DBCs or UI.
- **`dbc`** — turns `Frame`s into named values and back. Pure; no I/O, no clock, unit-testable.
- **`bus`** — owns the clock and the one live transport. Knows nothing about signals.
- **`app`** — the only mutable state. Reducer-shaped: `Action` in, state mutation out.
- **`ui`** — a pure function of `&App` and the current `Rect`. Never mutates state.

---

## 4. Interface abstraction and runtime switching

```rust
// transport/mod.rs
use embedded_can::Id;
use std::time::Duration;

/// An inline payload — 64 bytes, so classic and FD frames share one type and
/// the receive path allocates nothing. No `heapless` dependency needed.
#[derive(Clone, Copy)]
pub struct Payload { bytes: [u8; 64], len: u8 }   // Deref<Target = [u8]>

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub id: Id,        // Standard | Extended — not a bool
    pub data: Payload,
    pub echo: bool,    // gs_usb hands back our own TX; see §4.3
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("device not found: {0}")]         NotFound(String),
    #[error("permission denied: {0}")]        Permission(String),
    #[error("bitrate {0} unsupported here")]  Bitrate(u32),
    #[error("link down")]                     Disconnected,
    #[error(transparent)]                     Other(#[from] anyhow::Error),
}

/// One adapter. Blocking, single-threaded — the bus thread owns it exclusively.
pub trait Transport: Send {
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError>;
    /// `Ok(None)` on timeout — that is normal, not an error.
    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError>;
    fn describe(&self) -> String;
}
```

`Send` but not `Sync`, and never cloned: only the bus thread can touch it.

### 4.1 Describing a connection

`TransportSpec` is serialisable, so "reconnect to what I used last time" is a
config field, and the interface picker is a list of these.

```rust
// transport/spec.rs
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum TransportSpec {
    GsUsb { bus: u8, address: u8, vid: u16, pid: u16, bitrate: u32 },
    #[cfg(target_os = "linux")]
    SocketCan { iface: String },
    Slcan { port: String, bitrate: u32 },
    Virtual,
}

pub fn open(spec: &TransportSpec) -> Result<Box<dyn Transport>, TransportError> {
    Ok(match spec {
        TransportSpec::GsUsb { bus, address, vid, pid, bitrate } =>
            Box::new(gs_usb::GsUsb::open(*bus, *address, *vid, *pid, *bitrate)?),
        #[cfg(target_os = "linux")]
        TransportSpec::SocketCan { iface } =>
            Box::new(socketcan::SocketCan::open(iface)?),
        TransportSpec::Slcan { port, bitrate } =>
            Box::new(slcan::Slcan::open(port, *bitrate)?),
        TransportSpec::Virtual => Box::new(virt::Loopback::new()),
    })
}

/// Everything plugged in right now, ready to render as a picker list.
pub fn discover() -> Vec<(TransportSpec, String)> { /* discover.rs, §7 */ }
```

### 4.2 Switching while running

The entire switch is these fifteen lines in the bus thread. `self.transport`
is `Option<Box<dyn Transport>>`; assigning to it drops the previous adapter,
which closes its handle in `Drop`. Two interfaces cannot coexist.

```rust
// bus/mod.rs
fn handle(&mut self, cmd: Command) {
    match cmd {
        Command::Connect(spec) => {
            self.transport = None;                     // close the old one first
            self.cyclic.clear();                       // periods belong to a link
            match transport::open(&spec) {
                Ok(t) => {
                    let who = t.describe();
                    self.transport = Some(t);
                    let _ = self.tx.send(Event::Connected { spec, who });
                }
                Err(e) => { let _ = self.tx.send(Event::ConnectFailed(e.to_string())); }
            }
        }
        Command::Send { id, data }           => self.transmit(id, &data),
        Command::SetCyclic { key, id, data, period } => self.cyclic.set(key, id, data, period),
        Command::ClearCyclic(key)             => self.cyclic.clear_one(key),
        Command::ClearAllCyclic               => self.cyclic.clear(),
        Command::Shutdown                     => self.running = false,
    }
}
```

### 4.3 The scheduler

This is the part `can-bench.py` gets right and is worth keeping: **one thread does
TX and RX, and RX is drained continuously**, which is what stops gs_usb stalling its
own transmit path. The Rust version replaces the busy-wait with a real deadline.

```rust
pub fn run(mut self) {
    while self.running {
        // How long we may block: until the next cyclic frame is due.
        let budget = self.cyclic.next_due()
            .map(|due| due.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_millis(20))
            .min(Duration::from_millis(20));

        match self.rx_cmd.recv_timeout(budget) {
            Ok(cmd)                              => { self.handle(cmd); continue; }
            Err(RecvTimeoutError::Disconnected)  => break,
            Err(RecvTimeoutError::Timeout)       => {}
        }

        for (id, data) in self.cyclic.take_due(Instant::now()) {
            self.transmit(id, &data);
        }

        // Drain RX for the rest of the slice.
        let deadline = Instant::now() + Duration::from_millis(10);
        while Instant::now() < deadline {
            let Some(t) = self.transport.as_mut() else { break };
            match t.recv(Duration::from_millis(2)) {
                Ok(Some(f)) if f.echo => self.tx.send(Event::TxEcho(f)).ok(),
                Ok(Some(f))           => self.tx.send(Event::Rx(f, Instant::now())).ok(),
                Ok(None)              => break,
                Err(e)                => { self.tx.send(Event::BusError(e.to_string())).ok(); break }
            };
        }
    }
}
```

> `f.echo`: gs_usb returns every frame you transmit on the IN endpoint with
> `echo_id != 0xFFFF_FFFF`. Without the flag, every frame you send appears in the
> RX table as if the bus had answered you. `python-can` hides this; we must not.

### 4.4 gs_usb / candleLight, concretely

The connect sequence, so the "straightforward" promise holds: the user picks a
bitrate in Hz and we do the rest.

```rust
// transport/gs_usb.rs  — control requests, per the gs_usb protocol
const BREQ_HOST_FORMAT: u8 = 0;
const BREQ_BITTIMING:   u8 = 1;
const BREQ_MODE:        u8 = 2;
const BREQ_BT_CONST:    u8 = 4;
const BREQ_DEVICE_CONFIG: u8 = 5;
const MODE_START: u32 = 1;
const EP_IN: u8 = 0x81;
const EP_OUT: u8 = 0x02;

impl GsUsb {
    pub fn open(bus: u8, address: u8, vid: u16, pid: u16, bitrate: u32)
        -> Result<Self, TransportError>
    {
        let dev = rusb::devices()?.iter()
            .find(|d| d.bus_number() == bus && d.address() == address)
            .ok_or_else(|| TransportError::NotFound(format!("{vid:04x}:{pid:04x}")))?;
        let mut h = dev.open().map_err(|e| match e {
            rusb::Error::Access => TransportError::Permission(
                "no access to the USB device (Linux: install the udev rule; \
                 see `tuican --print-udev-rule`)".into()),
            other => TransportError::Other(other.into()),
        })?;
        #[cfg(target_os = "linux")]
        let _ = h.set_auto_detach_kernel_driver(true);   // if gs_usb.ko grabbed it
        h.claim_interface(0)?;

        h.write_control(0x41, BREQ_HOST_FORMAT, 1, 0, &0xEFBE_ADDEu32.to_le_bytes(), T)?;

        // Ask the device for its clock and segment limits, then solve for it.
        let mut bt = [0u8; 40];
        h.read_control(0xC1, BREQ_BT_CONST, 0, 0, &mut bt, T)?;
        let timing = BitTiming::solve(&BtConst::parse(&bt), bitrate)
            .ok_or(TransportError::Bitrate(bitrate))?;
        h.write_control(0x41, BREQ_BITTIMING, 0, 0, &timing.to_le_bytes(), T)?;

        h.write_control(0x41, BREQ_MODE, 0, 0, &mode_payload(MODE_START), T)?;
        Ok(Self { handle: h, bitrate })
    }
}

/// gs_host_frame: echo_id u32 | can_id u32 | dlc u8 | channel u8 | flags u8 | rsv u8 | data[8]
impl Transport for GsUsb {
    fn send(&mut self, f: &Frame) -> Result<(), TransportError> {
        let mut buf = [0u8; 20];
        buf[0..4].copy_from_slice(&0u32.to_le_bytes());          // our echo cookie
        buf[4..8].copy_from_slice(&raw_can_id(f.id).to_le_bytes());
        buf[8] = f.data.len() as u8;
        buf[12..12 + f.data.len()].copy_from_slice(&f.data);
        self.handle.write_bulk(EP_OUT, &buf, Duration::from_millis(500))?;
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError> {
        let mut buf = [0u8; 24];
        match self.handle.read_bulk(EP_IN, &mut buf, timeout) {
            Ok(n) if n >= 20 => Ok(Some(parse_host_frame(&buf))),  // sets .echo
            Ok(_) | Err(rusb::Error::Timeout) => Ok(None),
            Err(rusb::Error::NoDevice) => Err(TransportError::Disconnected),
            Err(e) => Err(TransportError::Other(e.into())),
        }
    }
}
```

`BitTiming::solve` — brute-force `brp` over the device's advertised range,
pick the sample point nearest 87.5 %, reject anything with error > 0.5 %.
Fifty lines, and it is the difference between "enter 500000" and "enter
prop_seg, phase_seg1, phase_seg2, sjw, brp".

---

## 5. Loading and decoding a DBC

`can-dbc` parses; it does not encode or decode. That work is ours, and it is the
single highest-risk module — bit layout is where CAN tools are wrong.

### 5.1 Our own model

Do not let `can-dbc` types leak past `dbc/load.rs`. The accessor names shift
between `can-dbc` releases, and an internal model is also what makes room for a
second front-end parser later (`.kcd`, `.sym`).

```rust
// dbc/mod.rs
pub struct Database {
    pub path: PathBuf,
    pub messages: Vec<MessageDef>,               // sorted by name
    by_id: HashMap<u32, usize>,
}

pub struct MessageDef {
    pub id: Id, pub name: String, pub len: usize,
    pub cycle_time_ms: Option<u32>,
    pub signals: Vec<SignalDef>,
}

pub struct SignalDef {
    pub name: String,
    pub start: u16, pub len: u16,
    pub big_endian: bool, pub signed: bool, pub float: bool,
    pub factor: f64, pub offset: f64,
    pub min: Option<f64>, pub max: Option<f64>,
    pub unit: String,
    pub choices: BTreeMap<i64, String>,
    pub mux: Mux,                                 // Plain | Multiplexor | In(Vec<u64>)
}

// dbc/load.rs — the only file that mentions can-dbc.
pub fn load(path: &Path) -> anyhow::Result<Database> {
    let bytes = std::fs::read(path)?;
    let dbc = can_dbc::Dbc::try_from(text.as_str())
        .map_err(|e| anyhow!("{}: not a valid DBC ({e})", path.display()))?;

    let messages: Vec<MessageDef> = dbc.messages.iter().filter_map(|m| {
        let id = message_id(m.id)?;          // skips the independent-signals pseudo-message
        Some(MessageDef {
            id, name: m.name.clone(), len: m.size as usize,
            cycle_time_ms: cycle_time(&dbc, m.id),      // GenMsgCycleTime attribute
            signals: m.signals.iter().map(|s| SignalDef {
                name: s.name.clone(),
                start: s.start_bit as u16,
                len:   s.size as u16,
                big_endian: matches!(s.byte_order, ByteOrder::BigEndian),
                signed:     matches!(s.value_type, ValueType::Signed),
                // A zero factor would make every encode a division by zero.
                factor: if s.factor == 0.0 { 1.0 } else { s.factor },
                offset: s.offset,
                choices: choices(&dbc, m.id, &s.name),
                mux: /* MultiplexIndicator → Mux */,
                ..
            }).collect(),
        })
    }).collect();

    Ok(Database::new(path.to_path_buf(), messages))   // sorts, builds the id index
}
```

Switching files at runtime is then: parse into a fresh `Database`, and on success
swap it in. Parse failure leaves the old one untouched — you cannot end up with no
database because you typo'd a path.

```rust
// app/mod.rs
fn load_dbc(&mut self, path: &Path) {
    match dbc::load(path) {
        Ok(db) => {
            self.db = Arc::new(db);
            self.values.clear();          // signal edits belong to the old file
            self.rx.redecode(&self.db);   // re-decode retained frames, keep counters
            self.msg_index = 0;
            self.status = format!("loaded {} ({} messages)", path.display(), self.db.messages.len());
        }
        Err(e) => self.status = format!("{e:#}"),   // old database still live
    }
}
```

### 5.2 The codec

Bit-by-bit rather than the `u64`-shift trick: it is obviously correct, it handles
Motorola's sawtooth numbering without a sign error, and it does not break at 8
bytes, so CAN FD works for free.

```rust
// dbc/codec.rs
fn extract(data: &[u8], start: u16, len: u16, big_endian: bool) -> u64 {
    let bit_at = |byte: usize, bit: u32| -> u64 {
        data.get(byte).map_or(0, |b| ((b >> bit) & 1) as u64)
    };
    let mut v = 0u64;
    for i in 0..len as u32 {
        if big_endian {
            // DBC big-endian start_bit is the MSB, numbered LSB-first within its
            // byte; the signal then walks forward in MSB-first bit order.
            let m = (start / 8) as u32 * 8 + (7 - (start % 8) as u32) + i;
            v = (v << 1) | bit_at((m / 8) as usize, 7 - (m % 8));
        } else {
            let idx = start as u32 + i;
            v |= bit_at((idx / 8) as usize, idx % 8) << i;
        }
    }
    v
}

fn insert(data: &mut [u8], start: u16, len: u16, big_endian: bool, raw: u64) {
    for i in 0..len as u32 {
        let (b, bit, src) = if big_endian {
            let m = (start / 8) as u32 * 8 + (7 - (start % 8) as u32) + i;
            ((m / 8) as usize, 7 - (m % 8), (raw >> (len as u32 - 1 - i)) & 1)
        } else {
            let idx = start as u32 + i;
            ((idx / 8) as usize, idx % 8, (raw >> i) & 1)
        };
        if let Some(byte) = data.get_mut(b) {
            *byte = (*byte & !(1 << bit)) | ((src as u8) << bit);
        }
    }
}

pub fn decode_signal(sig: &SignalDef, data: &[u8]) -> f64 {
    let raw = extract(data, sig.start, sig.len, sig.big_endian);
    let raw = if sig.signed && sig.len < 64 && (raw >> (sig.len - 1)) & 1 == 1 {
        (raw | (u64::MAX << sig.len)) as i64          // sign-extend
    } else {
        raw as i64
    };
    raw as f64 * sig.factor + sig.offset
}

pub fn encode(msg: &MessageDef, values: &HashMap<String, f64>)
    -> Result<Vec<u8>, CodecError>
{
    let mut data = vec![0u8; msg.len];
    for sig in mux::active_signals(msg, values) {          // §5.3
        let v = *values.get(&sig.name).ok_or(CodecError::Missing(sig.name.clone()))?;
        let raw = ((v - sig.offset) / sig.factor).round();
        let (lo, hi) = mux::representable_range(sig);
        if v < lo || v > hi {
            return Err(CodecError::OutOfRange { sig: sig.name.clone(), v, lo, hi });
        }
        insert(&mut data, sig.start, sig.len, sig.big_endian, raw as i64 as u64);
    }
    Ok(data)
}
```

Test it against the round-trip property (`decode(encode(v)) ≈ v` for every signal
of every message in a sample DBC, quantisation aside) plus a handful of
hand-computed Motorola vectors. `dbc/codec.rs` is pure, so this is a plain
`#[test]` with no hardware.

### 5.3 Two things carried over from the Python script

Both are load-bearing and both are easy to omit:

- **Representable range from the bit layout, not the declared min/max.** Real DBCs
  contradict themselves — the script's comment about a 7-bit signal at scale 0.1,
  offset −20 declaring a maximum of 125 is exactly the failure mode. Compute
  `[raw_lo·factor+offset, raw_hi·factor+offset]`, then narrow by the declared
  limits only where they fall inside it.
- **Multiplexor resolution.** Encode only the signals selected by the current
  multiplexor value, and seed the multiplexor with the lowest ID the message
  actually defines — 0 is frequently not one of them, which silently makes the
  message unsendable.

---

## 6. Reactive TUI

### 6.1 There is no resize handler

ratatui recomputes the whole layout from `frame.area()` every draw. `Resize` is
therefore only a *redraw trigger* plus a scroll-offset clamp. The layout adapts
because it is expressed as constraints, not as numbers.

```rust
// ui/layout.rs
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// Rects of the last frame, kept solely so the mouse can hit-test them.
#[derive(Default, Clone)]
pub struct LayoutMap {
    pub messages: Rect, pub signals: Rect, pub rx: Rect, pub status: Rect,
}

pub fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();

    // Below this, panes can't hold a useful row count; say so instead of
    // rendering unreadable slivers.
    if area.width < 60 || area.height < 12 {
        f.render_widget(
            Paragraph::new("terminal too small — need 60×12").centered(),
            area);
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(1),      // header
        Constraint::Percentage(45), // messages | signals
        Constraint::Min(6),         // rx table — absorbs the slack
        Constraint::Length(1),      // status
        Constraint::Length(1),      // key hints
    ]).split(area);

    // Narrow terminals stack the two top panes instead of squeezing them;
    // wide ones give messages a fixed, comfortable column.
    let top = if area.width >= 110 {
        Layout::horizontal([Constraint::Length(38), Constraint::Min(40)]).split(rows[1])
    } else if area.width >= 80 {
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(rows[1])
    } else {
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).split(rows[1])
    };

    app.layout = LayoutMap { messages: top[0], signals: top[1], rx: rows[2], status: rows[3] };

    // Clamp scroll to whatever height we actually got this frame.
    app.msg_scroll.clamp_to(top[0].height.saturating_sub(2) as usize, app.visible_messages().len());
    app.rx_scroll.clamp_to(rows[2].height.saturating_sub(3) as usize, app.rx.len());

    panes::header(f, rows[0], app);
    panes::messages(f, top[0], app);
    panes::signals(f, top[1], app);
    panes::rx(f, rows[2], app);          // columns drop out below width thresholds
    panes::status(f, rows[3], app);
    panes::hints(f, rows[4], app);
    if let Some(m) = &app.modal { panes::modal(f, area, m); }   // centred on `area`
}
```

The RX pane sheds columns rather than truncating: `decoded` below 100 columns
loses the smoothed period, below 80 loses the raw hex, and the name column is
elided in the middle (`ImuAcc…ature`) rather than at the end, where DBC names
differ.

### 6.2 The loop

```rust
// main.rs
fn run(term: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App,
       events: Receiver<Event>) -> anyhow::Result<()>
{
    const FRAME: Duration = Duration::from_millis(33);       // ~30 fps ceiling
    let mut next = Instant::now();
    loop {
        // Coalesce: a busy bus produces far more events than frames.
        while let Ok(ev) = events.try_recv() { app.on_bus_event(ev); }

        while crossterm::event::poll(Duration::ZERO)? {
            match crossterm::event::read()? {
                CEvent::Key(k) if k.kind == KeyEventKind::Press =>
                    app.update(input::map(k, app.mode)),
                CEvent::Mouse(m)  => app.update(mouse::map(m, &app.layout, app.mode)),
                CEvent::Resize(..) => app.dirty = true,       // next draw does the rest
                _ => {}
            }
        }
        if app.should_quit { return Ok(()); }

        term.draw(|f| ui::draw(f, app))?;

        next += FRAME;
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
}
```

Terminal setup and, importantly, teardown-on-panic — without the hook a panic
leaves the user in a raw-mode alternate screen with no echo:

```rust
fn setup() -> anyhow::Result<Terminal<CrosstermBackend<Stdout>>> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| { let _ = restore(); prev(info); }));
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout()))?)
}

fn restore() -> anyhow::Result<()> {
    execute!(stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
```

### 6.3 Mouse — additive only

Rule: **the mouse can only do what a key can already do, and never anything
destructive.** No click sends a frame. Nothing is mouse-only.

```rust
// ui/mouse.rs
pub fn map(ev: MouseEvent, l: &LayoutMap, mode: Mode) -> Action {
    if mode != Mode::Normal { return Action::None; }         // modal open: keyboard owns input
    let p = (ev.column, ev.row);

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => match hit(p, l) {
            Some((Pane::Messages, row)) => Action::FocusAndSelect(Pane::Messages, row),
            Some((Pane::Signals,  row)) => Action::FocusAndSelect(Pane::Signals,  row),
            Some((Pane::Rx,       row)) => Action::FocusAndSelect(Pane::Rx,       row),
            None => Action::None,
        },
        // Double-click == Enter: opens the value editor. Still a modal, still keyboard.
        MouseEventKind::Down(MouseButton::Left) if ev.modifiers.is_empty() => Action::None,
        MouseEventKind::ScrollDown => Action::ScrollIn(pane_at(p, l), 3),
        MouseEventKind::ScrollUp   => Action::ScrollIn(pane_at(p, l), -3),
        _ => Action::None,
    }
}

fn hit(p: (u16, u16), l: &LayoutMap) -> Option<(Pane, usize)> {
    for (pane, r) in [(Pane::Messages, l.messages), (Pane::Signals, l.signals), (Pane::Rx, l.rx)] {
        if r.contains(Position::new(p.0, p.1)) {
            let inner_top = r.y + 1 + pane.header_rows();   // border + column header
            return (p.1 >= inner_top).then(|| (pane, (p.1 - inner_top) as usize));
        }
    }
    None
}
```

Mouse capture swallows the terminal's own text selection, which people notice the
first time they try to copy a decoded line. So: **`F2` toggles capture**, the
status bar shows `mouse:on|off`, and the setting persists. `Action::ScrollIn`
targets the pane under the *cursor*, not the focused pane — scrolling a pane you
aren't focused on is the behaviour people expect and it leaves focus alone, so
keyboard navigation is never displaced by a stray wheel event.

That last rule needs one piece of state the first draft missed. Every pane keeps

```rust
pub struct Scroll { pub offset: usize, pub follow: bool }
```

`follow` starts set, so the view tracks the cursor on every frame. A wheel event
clears it, and the next keyboard move sets it again. Without it, "keep the cursor
visible" runs each frame and snaps the wheel straight back — the view refuses to
move at all while the cursor is off-screen. Three offsets, one per pane: routing
a scroll over the signals pane into the message list's offset is the other bug
this shape prevents.

---

## 7. User workflow

```
$ tuican
```

No arguments. What happens:

1. **Interface picker opens first**, populated by `discover()`:
   - USB devices matching the gs_usb/candleLight VID/PID table (`1d50:606f`
     candleLight, `1209:2323` CANable, …), shown by bus/address and product string;
   - on Linux, every `can*` in `/sys/class/net`, with its state and bitrate;
   - serial ports that look like a CAN adapter, for slcan;
   - `virtual (loopback)` always, so the tool is usable with nothing plugged in.

   The previously used interface is preselected. `Enter` connects. If a bitrate is
   needed, a second line offers the common set (125k / 250k / 500k / 1M), defaulting
   to the last one used. That is the whole connection step — the bit-timing solver
   in §4.4 means there is nothing else to answer.

   Failures are actionable, not `Errno 13`: a `Permission` error on Linux prints
   the exact udev rule and the path to write it to.

2. **DBC picker.** Last-used file preselected; otherwise a file browser rooted at
   the working directory, filtered to `*.dbc`. `Enter` loads it. One file is live at
   a time; `F3` reopens this picker at any point and hot-swaps (§5.1) without
   dropping the link.

3. **Main screen.** Three panes: message list (left), signal editor for the
   selected message (right), receive table (bottom). Header carries interface,
   bitrate, DBC name, TX/RX counters, and cyclic count.

4. **Working.** Keys are the Python script's, so the muscle memory transfers:

   | Key | |
   |---|---|
   | `Tab` / `S-Tab` | cycle panes |
   | `↑`/`↓`, `k`/`j`, `PgUp`/`PgDn`, `g`/`G` | move within a pane |
   | `/` | filter the message list (name or hex ID) |
   | `Enter` | edit the selected signal — range/enum hint shown inline |
   | `s` | send the selected message once |
   | `p` | toggle cyclic send; prompts for a period, defaulting to the DBC's `cycle_time` |
   | `x` | stop all cyclic sends |
   | `r` | raw send — `4E5 11 22 33` |
   | `c` | clear the receive table |
   | `F2` / `F3` / `F4` | mouse toggle / change DBC / change interface |
   | `?` | key help overlay |
   | `q` | quit |

   New relative to the script: `F4` reconnects to a different adapter without
   restarting; `F3` swaps DBC in place; the RX table marks a row stale (dimmed)
   when it exceeds 3× its own smoothed period, which is how you see a node drop
   off; frames with no matching DBC entry stay in the table as hex rather than
   disappearing.

5. **Exit.** `q` stops cyclic sends, closes the transport, restores the terminal,
   and writes the config (interface, bitrate, DBC path, per-message signal values)
   so the next launch is `tuican` `Enter` `Enter`.

Escape hatches for people who don't want the pickers:

```
tuican --dbc board.dbc --interface gs_usb --bitrate 1000000
tuican --dbc board.dbc --interface socketcan --channel can0
tuican --last                 # reconnect to the previous session and skip both pickers
```

---

## 8. Build order

1. `dbc/` + `transport/virt.rs` — decode/encode correct and unit-tested with no hardware.
2. `bus/` + `app/` + `ui/` against the loopback transport — the whole UI, testable.
3. `transport/gs_usb.rs` — the real target, and the riskiest (bit timing, echo frames).
4. `transport/socketcan.rs`, `transport/slcan.rs` — mechanical after the trait exists.
5. `discover.rs`, config persistence, pickers — the "straightforward" layer.

---

## 9. What is left

- **CAN FD.** `Payload` and the codec are 64-byte clean and tested there, so
  this is a transport-level change: gs_usb needs the `GS_CAN_MODE_FD` flag and a
  second, data-phase bit timing; SocketCAN needs `CanFdSocket`. Nothing above the
  transport moves.
- **Bus error and bus-off state.** SocketCAN surfaces error frames and gs_usb has
  them in its flags. Today both are reported as a line in the error area; a
  bus-off indicator in the header would be worth more.
- **Editing rules in the TUI.** They are loaded from TOML and reloaded with
  `F7`; the panel toggles them on and off but does not edit them. A visual graph
  editor would be a much larger piece of work, and the file round-trips fast
  enough that it has not been the bottleneck.
- **Merging several DBCs.** The constraint was one file at a time and that is
  what shipped, but `can-bench.py` merged several. `Database::new` already takes
  a `Vec<MessageDef>`, so this is a merge function that rejects duplicate frame
  ids — an addition, not a redesign.
- **Extended multiplexing (`SG_MUL_VAL_`).** `Mux::SelectedBy` holds a `Vec<u64>`
  precisely so this fits, but only the plain single-multiplexor form is populated
  today. Files using the extended form will show the extra signals as always
  present.
- **Hardware.** Everything below `transport::spec::open` has been exercised
  against the loopback only. `gs_usb.rs` is written to the protocol and its bit
  timing solver is unit-tested against a 48 MHz candleLight (exact at 125k, 250k,
  500k and 1M, sample points matching SocketCAN's), but no adapter was plugged
  into the machine this was written on. The same goes for `socketcan.rs`, which
  does not even compile on macOS by construction, and `slcan.rs`.

---

## 10. Round two: reacting, reconnecting, and getting around

Four things came out of the first real session on a MacBook with a candleLight
adapter. Each one changed the architecture in a small, specific way.

### 10.1 The link comes back on its own

Resetting the board dropped the USB device, and the tool left the user to
reconnect by hand. The bus thread now keeps the `TransportSpec` it opened, and a
fatal error routes through `lost_link()` rather than clearing everything:

```rust
fn lost_link(&mut self) {
    self.transport = None;                       // closes the adapter
    self.retry_backoff = FIRST_RETRY;            // 250 ms, doubling to 2 s
    self.retry_at = self.spec.as_ref().map(|_| Instant::now() + FIRST_RETRY);
    self.emit(Event::Disconnected { retrying: self.retry_at.is_some() });
}
```

The periodic sends are deliberately **not** cleared. They are held, and
`Cyclic::rearm` restarts their schedules from the moment the link returns, so
they resume at their own rate rather than all firing at once to catch up. The
run loop simply does not call `take_due` while the transport is `None`, which is
also what stops a "not connected" error per frame per entry.

One detail matters more than the retry loop: a board that has just been reset
re-enumerates at a **different USB address**. Locating it by bus and address, as
the first version did, is precisely what fails. `GsUsb::open` now filters by
vendor and product id first and prefers the original slot within that set, so it
finds the same adapter wherever it came back.

`Event::Connected` carries `resumed: bool`. A fresh connect clears the periodic
sends; a resumed one does not.

### 10.2 Rules: a graph, not a script

The ask was "change this message to this if this message changes to that", and
chained "also in parallel, not only series, like a graph of interactions".

Two decisions make that a graph rather than a list:

1. **Rules are independent and all are evaluated.** Any number can react to the
   same frame. That is the parallel case, and it falls out of not stopping at
   the first match.
2. **A condition can read a signal we are transmitting** (`source = "tx"`), not
   only one arriving from the bus. One rule's action then satisfies another
   rule's condition. `App::run_rules` re-evaluates until nothing new fires, so
   the chain completes within a single tick instead of one link per frame.

```rust
for pass in 0..rules::MAX_PASSES {
    let fired = self.rules.pass(&self.rx, &self.values);
    if fired.is_empty() { self.rules.looped = false; return; }
    for (name, actions) in fired {
        for act in actions { self.apply_act(&name, &act); }
    }
    if pass + 1 == rules::MAX_PASSES { /* a cycle: say so, stop */ }
}
```

Cascading and cycles are the same mechanism, so the pass cap is not optional.
Two rules that flip each other's value never settle, and a test asserts the
evaluation terminates and reports rather than eating the frame budget.

Rules fire on the **rising edge**. Without `Rule::holding`, a rule watching
`state == 2` would fire on every frame for as long as the state stayed at 2,
which is not what "if this message changes to that" means.

Evaluation lives on the UI side, like decoding, because it needs decoded signal
values and the transmit-side state. The bus thread still knows nothing about a
DBC. `RxRow` gained a `values: HashMap<String, f64>` so conditions have numbers
to compare rather than the formatted display string.

Names are resolved against the DBC at load time. A rule naming a signal that
does not exist would otherwise never fire and never explain itself, so each
broken rule carries its own `warning` and shows as `bad` in red in the panel.

### 10.3 Two panels sharing one band

The one-line cyclic strip could only fit names. `F5` opens a real panel with the
id, period, count and the bytes actually going out; `F6` does the same for the
rules. Both are focusable panes, but only while open:

```rust
pub fn panes(&self) -> Vec<Pane> {
    Pane::ALL.into_iter().filter(|p| match p {
        Pane::Cyclic => self.show_cyclic_panel,
        Pane::Rules => self.show_rules_panel,
        _ => true,
    }).collect()
}
```

They share one band below the receive table, side by side above 120 columns and
stacked below it, which is the same width-driven rule the top panes already use.

Building this surfaced a latent rendering bug: the receive pane drew its column
header *over* the first row of its own list, hiding an entry whenever the list
was full. Header and body now get separate rects from `with_header`, and the
three parts of such a pane never share a row.

### 10.4 Motions and the filter

`j`, `k`, `gg`, `G`, `42G`, `5j`, `ctrl-d`/`u`/`f`/`b`, and `H`/`M`/`L`. Counts
and the `gg` prefix need state that a pure `KeyEvent -> Action` mapping does not
have, so `input::map` now also takes the pending state, and the reducer owns the
count. `h` and `l` switch panes: there is no horizontal cursor for them to move,
and pane switching is what a hand on `hjkl` wants next.

`/` now clears the previous filter instead of pre-filling it, because
re-filtering was cancelling and retyping every time. `esc` restores what was
there, so an accidental `/` is not destructive.

---

## 10. As built: where this differs from the first draft

Recorded because the code is the authority and the reasons are not obvious.

| Change | Why |
|---|---|
| `Frame` lost its `fd` flag; `Command::Disconnect`, `canid::from_packed` and a few helpers were dropped | Nothing read them. Speculative surface that no path exercises is dead code, not a feature. |
| Per-pane `Scroll { offset, follow }` | See §6.3. The wheel could not move a view at all without it. |
| Prompt defaults are placeholders, never pre-filled into the buffer | Pre-filling meant typing *appended*: entering `250` over a default of `100` armed a 100250 ms period. Found by measuring the actual send rate, not by reading the code. |
| DBC discovery walks three levels, not one | The common layout is `dbc/<network>/<name>.dbc`. A one-level scan found nothing in a real repo. |
| Header and receive pane shed whole fields by width | Clipping one field mid-word looks broken; dropping the least important field does not. |
| Rows clip at the tail; only names within a column elide in the middle | Middle-eliding a composed row cuts through the fixed-width columns. |
| §6.1's claim that middle-elision keeps DBC names distinguishable | It does not, and a test proved it: `TsacCellboard1Voltage` and `TsacCellboard2Voltage` collide at 14 columns, because the digit that separates them *is* in the middle. Elision is readability; the id column is what identifies a row. |
| Startup pickers are chosen independently | `--interface` used to still show the interface picker after the DBC one. |
| Cyclic sends remember the period the user chose | Editing a signal re-armed the send at the DBC's `GenMsgCycleTime` instead, silently changing the rate. |
