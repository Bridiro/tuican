//! The one thread allowed to touch the adapter.
//!
//! It owns the transport, the clock, and the periodic-send table. It holds no
//! reference to the DBC or to the UI, and the UI holds no reference to it —
//! everything crosses as [`Command`] and [`Event`] values, so there is no lock
//! anywhere in the program.

pub mod command;
pub mod cyclic;
pub mod event;

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};

use crate::transport::{self, Frame, Transport, TransportSpec};
use command::Command;
use cyclic::Cyclic;
use event::{Event, Stats};

/// How long a single pass may spend draining the receive queue.
///
/// Draining continuously is not an optimisation: it is what stops a gs_usb
/// adapter backing up and stalling its own transmit path.
const DRAIN_SLICE: Duration = Duration::from_millis(10);
const IDLE_SLICE: Duration = Duration::from_millis(20);
const STATS_EVERY: Duration = Duration::from_millis(200);

pub struct Bus {
    transport: Option<Box<dyn Transport>>,
    cyclic: Cyclic,
    commands: Receiver<Command>,
    events: Sender<Event>,
    stats: Stats,
    stats_sent: Instant,
    running: bool,
}

pub fn spawn(commands: Receiver<Command>, events: Sender<Event>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("can-bus".into())
        .spawn(move || {
            Bus {
                transport: None,
                cyclic: Cyclic::default(),
                commands,
                events,
                stats: Stats::default(),
                stats_sent: Instant::now(),
                running: true,
            }
            .run()
        })
        .expect("spawn bus thread")
}

impl Bus {
    fn run(mut self) {
        while self.running {
            // Block only until the next periodic frame is due.
            let budget = self
                .cyclic
                .next_due()
                .map(|due| due.saturating_duration_since(Instant::now()))
                .unwrap_or(IDLE_SLICE)
                .min(IDLE_SLICE);

            match self.commands.recv_timeout(budget) {
                Ok(cmd) => {
                    self.handle(cmd);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }

            for (id, data) in self.cyclic.take_due(Instant::now()) {
                self.transmit(id, &data);
            }
            self.drain();
            self.report();
        }
        // Dropping the transport takes the adapter off the bus.
        self.transport = None;
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Connect(spec) => self.connect(spec),
            Command::Send { id, data } => {
                self.transmit(id, &data);
                self.report_now();
            }
            Command::SetCyclic { key, id, data, period } => {
                self.cyclic.set(key, id, data, period)
            }
            Command::ClearCyclic(key) => self.cyclic.remove(&key),
            Command::ClearAllCyclic => self.cyclic.clear(),
            Command::Shutdown => self.running = false,
        }
    }

    fn connect(&mut self, spec: TransportSpec) {
        // Close the old adapter *before* opening the new one. Assigning to this
        // one field is the whole of the "only one interface is ever live" rule.
        self.transport = None;
        self.cyclic.clear();
        self.stats = Stats::default();

        match transport::spec::open(&spec) {
            Ok(t) => {
                let who = t.describe();
                self.transport = Some(t);
                tracing::info!(%who, "connected");
                self.emit(Event::Connected { spec, who });
            }
            Err(e) => {
                tracing::warn!(error = %e, "connect failed");
                self.emit(Event::ConnectFailed(e.to_string()));
            }
        }
    }

    fn transmit(&mut self, id: embedded_can::Id, data: &crate::transport::Payload) {
        let Some(t) = self.transport.as_mut() else {
            self.emit(Event::BusError("not connected".into()));
            return;
        };
        let frame = Frame { id, data: *data, echo: false };
        match t.send(&frame) {
            Ok(()) => self.stats.tx += 1,
            Err(e) => {
                self.stats.tx_errors += 1;
                let fatal = matches!(e, transport::TransportError::Disconnected);
                self.emit(Event::BusError(format!("tx {}: {e}", crate::canid::display(id))));
                if fatal {
                    self.transport = None;
                    self.cyclic.clear();
                    self.emit(Event::Disconnected);
                }
            }
        }
    }

    fn drain(&mut self) {
        let deadline = Instant::now() + DRAIN_SLICE;
        while Instant::now() < deadline {
            let Some(t) = self.transport.as_mut() else { return };
            match t.recv(Duration::from_millis(2)) {
                Ok(Some(frame)) if frame.echo => self.stats.tx_echo += 1,
                Ok(Some(frame)) => {
                    self.stats.rx += 1;
                    self.emit(Event::Rx(frame, Instant::now()));
                }
                Ok(None) => return,
                Err(transport::TransportError::Disconnected) => {
                    self.transport = None;
                    self.cyclic.clear();
                    self.emit(Event::Disconnected);
                    return;
                }
                Err(e) => {
                    self.emit(Event::BusError(e.to_string()));
                    return;
                }
            }
        }
    }

    fn report(&mut self) {
        if self.stats_sent.elapsed() >= STATS_EVERY {
            self.report_now();
        }
    }

    fn report_now(&mut self) {
        self.stats_sent = Instant::now();
        self.stats.cyclic = self.cyclic.counts();
        let stats = self.stats.clone();
        self.emit(Event::Stats(stats));
    }

    fn emit(&self, event: Event) {
        // A closed channel means the UI is already gone; nothing to do about it.
        let _ = self.events.send(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canid;
    use crate::transport::Payload;

    /// The scheduler must keep firing, not just fire once. Regression test for a
    /// periodic send that stopped after its first frame.
    #[test]
    fn cyclic_keeps_firing_at_the_requested_rate() {
        let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
        let (evt_tx, evt_rx) = crossbeam_channel::unbounded();
        let handle = spawn(cmd_rx, evt_tx);

        cmd_tx.send(Command::Connect(TransportSpec::Virtual)).unwrap();
        cmd_tx
            .send(Command::SetCyclic {
                key: "M".into(),
                id: canid::make(0x123, false).unwrap(),
                data: Payload::new(&[1, 2, 3]),
                period: Duration::from_millis(20),
            })
            .unwrap();

        std::thread::sleep(Duration::from_millis(500));
        cmd_tx.send(Command::Shutdown).unwrap();
        handle.join().unwrap();

        let tx = evt_rx
            .try_iter()
            .filter_map(|e| match e {
                Event::Stats(s) => Some(s.tx),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        // 500 ms at 20 ms is 25; allow generous slack for a loaded CI machine,
        // but "1" must fail.
        assert!(tx >= 10, "only {tx} frames in 500 ms at a 20 ms period");
    }
}
