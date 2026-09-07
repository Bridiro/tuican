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

/// A loopback that drops the link once, so the reconnect path can be tested
/// without unplugging real hardware. Test builds only.
#[cfg(test)]
pub mod flaky {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// How many more times an open transport should fail. Each failure costs
    /// one, so `arm(1)` reproduces a single board reset.
    static FAILURES_LEFT: AtomicU32 = AtomicU32::new(0);

    pub fn arm(failures: u32) {
        FAILURES_LEFT.store(failures, Ordering::SeqCst);
    }

    #[derive(Default)]
    pub struct Flaky {
        sent: u32,
    }

    impl Transport for Flaky {
        fn send(&mut self, _frame: &Frame) -> Result<(), TransportError> {
            self.sent += 1;
            // Survive a few frames first, so the test sees traffic before the
            // drop and can tell "never worked" from "worked, then dropped".
            if self.sent > 3
                && FAILURES_LEFT
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
            {
                return Err(TransportError::Disconnected);
            }
            Ok(())
        }

        fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError> {
            std::thread::sleep(timeout.min(Duration::from_millis(2)));
            Ok(None)
        }

        fn describe(&self) -> String {
            "flaky test link".into()
        }
    }
}
