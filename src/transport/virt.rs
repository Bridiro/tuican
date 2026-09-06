//! A loopback bus, so the whole tool is usable and testable with nothing plugged
//! in. Behaves like SocketCAN's `vcan`: whatever you send comes back as receive
//! traffic.

use std::collections::VecDeque;
use std::time::Duration;

use super::{Frame, Transport, TransportError};

#[derive(Default)]
pub struct Loopback {
    queue: VecDeque<Frame>,
}

impl Loopback {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Transport for Loopback {
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        let mut echoed = *frame;
        echoed.echo = false; // arrives as if another node had sent it
        self.queue.push_back(echoed);
        Ok(())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError> {
        match self.queue.pop_front() {
            Some(f) => Ok(Some(f)),
            None => {
                // Nothing to report; still burn the slice so the bus thread's
                // loop paces itself the same way it does against real hardware.
                std::thread::sleep(timeout.min(Duration::from_millis(5)));
                Ok(None)
            }
        }
    }

    fn describe(&self) -> String {
        "virtual loopback".into()
    }
}
