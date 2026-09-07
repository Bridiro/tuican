# tuican

A terminal CAN bench: pick an adapter, load a DBC, send and watch traffic.

```
cargo run --release
```

No arguments needed. tuican finds the DBC files below the working directory and
the adapters plugged in, and asks which you want. The second launch remembers
both.

```
┌ ● candleLight @ 1 Mbit/s │ primary.dbc · 165 msgs │ tx 12 rx 4183 │ ids 22 ─┐
│ messages ──────────────────── signals  BrakesEBS (8 B) ────────────────────│
│  0x005 BrakesEBS             pressureTank1            4.2 bar               │
│ ~0x332 ControlMapsSet        pressureTank2            4.1 bar               │
│  0x4DB CoolingCircuitTemp…   pressureLine1            0 bar                 │
│ receive  22 ids ───────────────────────────────────────────────────────────│
│        id name              count      ms  data            decoded         │
│     0x4DB CoolingCircuit…    1042    10.1  8C 0A 91 0A     inlet=27 degC …  │
└────────────────────────────────────────────────────────────────────────────┘
```

## Keys

| | |
|---|---|
| `tab` / `shift-tab`, `h` `l` | move between panes |
| `j` `k`, `↑ ↓` | move within a pane |
| `5j`, `12k` | repeat a motion |
| `gg` / `G` / `42G` | top, bottom, that row |
| `ctrl-d` / `ctrl-u` | half page down / up |
| `ctrl-f` / `ctrl-b` | page down / up |
| `H` / `M` / `L` | top, middle, bottom of the screen |
| `/` | filter messages by name or hex id (starts empty each time) |
| `enter` | edit the selected signal, or act on the panel row |
| `s` | send the selected message once |
| `p` | start or stop sending it periodically |
| `d` | stop the periodic send under the cursor |
| `x` | stop every periodic send |
| `r` | raw send, e.g. `4E5 11 22 33` |
| `c` | clear the receive table |
| `F2` | mouse capture on/off (off restores terminal text selection) |
| `F3` | load a different DBC, without dropping the link |
| `F4` | connect to a different interface |
| `F5` | cyclic panel: everything going out, with periods and counts |
| `F6` | rules panel |
| `F7` | reload the rules file |
| `?` | key help |
| `q` | quit |

`/` clears the previous filter, so re-filtering is typing a new search rather
than cancelling and retyping. `esc` puts the old one back.

The mouse is optional and additive: click selects, the wheel scrolls the pane
under the pointer without moving keyboard focus. Nothing is mouse-only, and no
click ever sends a frame.

## Rules

A rule watches signals and, when its condition becomes true, changes what you
are sending. It is there so that simulating a board is not a sequence of manual
keystrokes.

```toml
[[rule]]
name = "enable on ready"
all = [
  { message = "InverterTelemetry", signal = "inverterReady", op = "eq", value = 1 },
]
then = [
  { action = "set", message = "InverterSetpoints", signal = "enableInverter", value = 1 },
  { action = "cyclic", message = "InverterSetpoints", period_ms = 10 },
]
```

Rules fire on the rising edge, so that one fires once when the inverter reports
ready, not on every frame while it stays ready.

Rules are independent, so any number of them can react to the same frame. A rule
can also watch a signal you are *sending* (`source = "tx"`), which is how one
rule's action satisfies another rule's condition within the same tick. Running
the two together gives a graph of reactions rather than a chain, and a cycle in
that graph is detected and reported instead of spinning.

Operators are `eq`, `ne`, `lt`, `le`, `gt`, `ge` and `changed`. Actions are
`set`, `send`, `cyclic` and `stop`. Every message and signal name is checked
against the loaded DBC when the file loads, and anything the DBC does not define
is flagged in the rules panel rather than silently never matching.

Put the file next to your DBC as `tuican.rules.toml` and tuican finds it, or
pass `--rules FILE`. `F6` shows the panel, `enter` toggles a rule, `F7` reloads
the file without restarting. A full commented template is in
[examples/tuican.rules.toml](examples/tuican.rules.toml).

## Reconnecting

If the board is reset while connected, the link drops. tuican notices, reopens
it by itself, and puts the periodic sends back, so a reset costs you nothing.
The header shows `◐ link lost, reconnecting` while it retries. A gs_usb adapter
that comes back at a different USB address is found again by its vendor and
product ids.

## Adapters

One is live at a time; `F4` switches without restarting.

| | |
|---|---|
| **gs_usb / candleLight** | over libusb. The path that works on macOS. Pick a bitrate in the picker — tuican asks the device for its clock and solves the bit timing. |
| **SocketCAN** | Linux only. Bring the link up first: `sudo ip link set can0 up type can bitrate 500000`. |
| **slcan** | Lawicel ASCII adapters over a serial port. |
| **virtual** | Loopback. Always offered, so the tool is usable with nothing plugged in. |

On Linux, `tuican --print-udev-rule` prints the rule that fixes the usual
permission error on USB adapters.

## Flags

Every one is optional and has a discovery fallback.

```
tuican --dbc board.dbc
tuican --dbc board.dbc --rules car.rules.toml
tuican --dbc board.dbc --interface socketcan --channel can0
tuican --interface gs_usb --bitrate 1000000
tuican --last                    # reconnect to the previous session, no pickers
```

## Notes

- Logs go to a file, never the screen: `RUST_LOG=debug tuican`, then read
  `tuican.log` under your platform's state directory.
- Config (last interface, bitrate, DBC, rules file, mouse setting) lives in
  `tuican/config.toml` under your platform's config directory.
- Classic CAN only for now. The payload type and the codec are already 64-byte
  clean, so CAN FD is a transport-level change; see DESIGN.md.

Architecture and the reasoning behind it: [DESIGN.md](DESIGN.md).
