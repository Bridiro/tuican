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
| `tab` / `shift-tab` | move between panes |
| `↑ ↓` `k` `j`, `pgup`/`pgdn`, `g`/`G` | move within a pane |
| `/` | filter messages by name or hex id |
| `enter` | edit the selected signal |
| `s` | send the selected message once |
| `p` | start or stop sending it periodically |
| `x` | stop every periodic send |
| `r` | raw send, e.g. `4E5 11 22 33` |
| `c` | clear the receive table |
| `F2` | mouse capture on/off (off restores terminal text selection) |
| `F3` | load a different DBC, without dropping the link |
| `F4` | connect to a different interface |
| `?` | key help |
| `q` | quit |

The mouse is optional and additive: click selects, the wheel scrolls the pane
under the pointer without moving keyboard focus. Nothing is mouse-only, and no
click ever sends a frame.

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
tuican --dbc board.dbc --interface socketcan --channel can0
tuican --interface gs_usb --bitrate 1000000
tuican --last                    # reconnect to the previous session, no pickers
```

## Notes

- Logs go to a file, never the screen: `RUST_LOG=debug tuican`, then read
  `tuican.log` under your platform's state directory.
- Config (last interface, bitrate, DBC, mouse setting) lives in
  `tuican/config.toml` under your platform's config directory.
- Classic CAN only for now. The payload type and the codec are already 64-byte
  clean, so CAN FD is a transport-level change; see DESIGN.md.

Architecture and the reasoning behind it: [DESIGN.md](DESIGN.md).
