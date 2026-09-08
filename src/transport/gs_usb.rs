//! candleLight / gs_usb over libusb.
//!
//! This is the only transport that works on macOS: there is no kernel driver
//! for these adapters there, so libusb claims the interface directly. On Linux
//! the same devices are usually already bound to `gs_usb.ko` and exposed as a
//! SocketCAN interface; we detach the kernel driver rather than fight it, but
//! prefer SocketCAN there when it is available.
//!
//! Protocol per the mainline `gs_usb` driver and the candleLight firmware.

use std::time::Duration;

use rusb::{DeviceHandle, GlobalContext};

use super::{Frame, Payload, Transport, TransportError};
use crate::canid;

// Vendor control requests.
const BREQ_HOST_FORMAT: u8 = 0;
const BREQ_BITTIMING: u8 = 1;
const BREQ_MODE: u8 = 2;
const BREQ_BT_CONST: u8 = 4;

const RT_OUT: u8 = 0x41; // host->device, vendor, interface
const RT_IN: u8 = 0xC1; // device->host, vendor, interface

const MODE_RESET: u32 = 0;
const MODE_START: u32 = 1;

const EP_IN: u8 = 0x81;
const EP_OUT: u8 = 0x02;

const HOST_MAGIC: u32 = 0x0000_BEEF; // byte-order probe the firmware expects
const ECHO_NONE: u32 = 0xFFFF_FFFF; // echo_id of a genuinely received frame
const HOST_FRAME_LEN: usize = 20;
const CAN_EFF_FLAG: u32 = 0x8000_0000;
const CAN_RTR_FLAG: u32 = 0x4000_0000;
const CAN_ERR_FLAG: u32 = 0x2000_0000;

const CTRL_TIMEOUT: Duration = Duration::from_millis(500);

pub const PERMISSION_HINT: &str = "cannot claim the USB device. On Linux, install a udev rule \
     (run `tuican --print-udev-rule`) and replug; on macOS, unplug and replug the adapter.";

/// Adapters that speak gs_usb, for the interface picker.
pub const KNOWN: &[(u16, u16, &str)] = &[
    (0x1D50, 0x606F, "candleLight"),
    (0x1209, 0x2323, "CANable"),
    (0x1CD2, 0x606F, "CANable2"),
    (0x16D0, 0x117E, "CES CANext"),
    (0x1D50, 0x60A1, "cannectivity"),
];

pub fn describe_ids(vid: u16, pid: u16) -> String {
    KNOWN
        .iter()
        .find(|(v, p, _)| *v == vid && *p == pid)
        .map(|(_, _, name)| (*name).to_string())
        .unwrap_or_else(|| format!("{vid:04x}:{pid:04x}"))
}

/// What the device says it can do, from `GS_USB_BREQ_BT_CONST`.
#[derive(Clone, Copy, Debug)]
pub struct BtConst {
    pub fclk_can: u32,
    pub tseg1_min: u32,
    pub tseg1_max: u32,
    pub tseg2_min: u32,
    pub tseg2_max: u32,
    pub sjw_max: u32,
    pub brp_min: u32,
    pub brp_max: u32,
    pub brp_inc: u32,
}

impl BtConst {
    fn parse(b: &[u8; 40]) -> Self {
        let w = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Self {
            // b[0..4] is the feature bitmap; we do not gate on it yet.
            fclk_can: w(4),
            tseg1_min: w(8),
            tseg1_max: w(12),
            tseg2_min: w(16),
            tseg2_max: w(20),
            sjw_max: w(24),
            brp_min: w(28),
            brp_max: w(32),
            brp_inc: w(36).max(1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitTiming {
    pub prop_seg: u32,
    pub phase_seg1: u32,
    pub phase_seg2: u32,
    pub sjw: u32,
    pub brp: u32,
}

impl BitTiming {
    fn to_le_bytes(self) -> [u8; 20] {
        let mut out = [0u8; 20];
        for (i, v) in [
            self.prop_seg,
            self.phase_seg1,
            self.phase_seg2,
            self.sjw,
            self.brp,
        ]
        .into_iter()
        .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out
    }

    /// Solve for a bitrate, so the user types `500000` and never sees a segment.
    ///
    /// Same sample-point targets SocketCAN uses, so a bus configured with `ip
    /// link` and one configured here agree.
    pub fn solve(bt: &BtConst, bitrate: u32) -> Option<Self> {
        if bitrate == 0 || bt.fclk_can == 0 {
            return None;
        }
        let target_sp = if bitrate > 800_000 {
            750
        } else if bitrate > 500_000 {
            800
        } else {
            875
        };

        let mut best: Option<(u64, u32, Self)> = None; // (bitrate error, |sp error|, timing)
        let mut brp = bt.brp_min.max(1);
        while brp <= bt.brp_max {
            // Total time quanta per bit at this prescaler.
            let nbt = (bt.fclk_can as f64 / (bitrate as f64 * brp as f64)).round() as u32;
            if nbt >= 1 + bt.tseg1_min + bt.tseg2_min && nbt <= 1 + bt.tseg1_max + bt.tseg2_max {
                let actual = bt.fclk_can as f64 / (brp as f64 * nbt as f64);
                let err = ((actual - bitrate as f64).abs() / bitrate as f64 * 1e9) as u64;

                // Place the sample point, then clamp both segments into range.
                let mut tseg1 = ((nbt as u64 * target_sp) / 1000) as u32;
                tseg1 = tseg1.saturating_sub(1).clamp(bt.tseg1_min, bt.tseg1_max);
                let mut tseg2 = nbt.saturating_sub(1).saturating_sub(tseg1);
                if tseg2 < bt.tseg2_min {
                    tseg2 = bt.tseg2_min;
                    tseg1 = nbt.saturating_sub(1).saturating_sub(tseg2);
                }
                if tseg1 >= bt.tseg1_min && tseg1 <= bt.tseg1_max && tseg2 <= bt.tseg2_max {
                    let sp = ((1 + tseg1) as u64 * 1000 / nbt as u64) as u32;
                    let sp_err = sp.abs_diff(target_sp as u32);
                    // The hardware only sees prop_seg + phase_seg1; split evenly.
                    let phase_seg1 = tseg1.div_ceil(2).max(1);
                    let cand = Self {
                        prop_seg: tseg1 - phase_seg1,
                        phase_seg1,
                        phase_seg2: tseg2,
                        sjw: bt.sjw_max.min(tseg2).max(1),
                        brp,
                    };
                    if best
                        .as_ref()
                        .is_none_or(|(e, s, _)| (err, sp_err) < (*e, *s))
                    {
                        best = Some((err, sp_err, cand));
                    }
                }
            }
            brp += bt.brp_inc;
        }
        // Reject anything worse than 0.5%: a bus at that error will not stay up.
        best.filter(|(err, _, _)| *err <= 5_000_000)
            .map(|(_, _, t)| t)
    }
}

pub struct GsUsb {
    handle: DeviceHandle<GlobalContext>,
    name: String,
    bitrate: u32,
    /// Incremented per send, purely so the echo is recognisable in a log.
    echo_seq: u32,
}

impl GsUsb {
    pub fn open(
        bus: u8,
        address: u8,
        vid: u16,
        pid: u16,
        bitrate: u32,
    ) -> Result<Self, TransportError> {
        // Match on vendor/product, preferring the exact slot we were given.
        // A board that has just been reset comes back at a different address,
        // and the old one may since have been handed to something else, so
        // neither "the same slot" nor "the same ids" is sufficient alone.
        let devices = rusb::devices().map_err(usb_err)?;
        let matching: Vec<_> = devices
            .iter()
            .filter(|d| {
                d.device_descriptor()
                    .is_ok_and(|desc| desc.vendor_id() == vid && desc.product_id() == pid)
            })
            .collect();
        let device = matching
            .iter()
            .find(|d| d.bus_number() == bus && d.address() == address)
            .or_else(|| matching.first())
            .ok_or_else(|| TransportError::NotFound(describe_ids(vid, pid)))?
            .clone();

        let handle = device.open().map_err(usb_err)?;
        // Linux only; everywhere else this is a no-op and the error is expected.
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle.claim_interface(0).map_err(usb_err)?;

        let name = device
            .device_descriptor()
            .ok()
            .and_then(|d| handle.read_product_string_ascii(&d).ok())
            .unwrap_or_else(|| describe_ids(vid, pid));

        let mut me = Self {
            handle,
            name,
            bitrate,
            echo_seq: 0,
        };
        me.configure(bitrate)?;
        Ok(me)
    }

    fn configure(&mut self, bitrate: u32) -> Result<(), TransportError> {
        // 1. Tell the firmware our byte order.
        self.handle
            .write_control(
                RT_OUT,
                BREQ_HOST_FORMAT,
                1,
                0,
                &HOST_MAGIC.to_le_bytes(),
                CTRL_TIMEOUT,
            )
            .map_err(usb_err)?;

        // 2. Ask what it can do, and solve for the requested bitrate.
        let mut raw = [0u8; 40];
        self.handle
            .read_control(RT_IN, BREQ_BT_CONST, 0, 0, &mut raw, CTRL_TIMEOUT)
            .map_err(usb_err)?;
        let bt = BtConst::parse(&raw);
        let timing = BitTiming::solve(&bt, bitrate).ok_or(TransportError::Bitrate(bitrate))?;
        tracing::info!(?bt, ?timing, bitrate, "gs_usb bit timing");
        self.handle
            .write_control(
                RT_OUT,
                BREQ_BITTIMING,
                0,
                0,
                &timing.to_le_bytes(),
                CTRL_TIMEOUT,
            )
            .map_err(usb_err)?;

        // 3. Go.
        self.set_mode(MODE_START)
    }

    fn set_mode(&self, mode: u32) -> Result<(), TransportError> {
        let mut payload = [0u8; 8];
        payload[..4].copy_from_slice(&mode.to_le_bytes()); // flags stay zero: classic CAN
        self.handle
            .write_control(RT_OUT, BREQ_MODE, 0, 0, &payload, CTRL_TIMEOUT)
            .map_err(usb_err)?;
        Ok(())
    }
}

impl Transport for GsUsb {
    fn send(&mut self, frame: &Frame) -> Result<(), TransportError> {
        if frame.data.len() > 8 {
            return Err(TransportError::Other(
                "classic CAN carries at most 8 bytes".into(),
            ));
        }
        self.echo_seq = self.echo_seq.wrapping_add(1) & 0x00FF_FFFF;

        let mut buf = [0u8; HOST_FRAME_LEN];
        buf[0..4].copy_from_slice(&self.echo_seq.to_le_bytes());
        buf[4..8].copy_from_slice(&packed_id(frame.id).to_le_bytes());
        buf[8] = frame.data.len() as u8; // can_dlc
        buf[9] = 0; // channel
        buf[12..12 + frame.data.len()].copy_from_slice(&frame.data);

        match self
            .handle
            .write_bulk(EP_OUT, &buf, Duration::from_millis(500))
        {
            Ok(_) => Ok(()),
            Err(rusb::Error::NoDevice) => Err(TransportError::Disconnected),
            Err(e) => Err(usb_err(e)),
        }
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Frame>, TransportError> {
        // libusb reads a zero timeout as "block forever"; never let that through.
        let timeout = timeout.max(Duration::from_millis(1));
        let mut buf = [0u8; 32]; // room for the optional hardware timestamp
        match self.handle.read_bulk(EP_IN, &mut buf, timeout) {
            Ok(n) if n >= HOST_FRAME_LEN => Ok(parse_host_frame(&buf)),
            Ok(_) => Ok(None),
            Err(rusb::Error::Timeout) => Ok(None),
            Err(rusb::Error::NoDevice) => Err(TransportError::Disconnected),
            Err(e) => Err(usb_err(e)),
        }
    }

    fn describe(&self) -> String {
        format!("{} @ {}", self.name, crate::fmt_bitrate(self.bitrate))
    }
}

impl Drop for GsUsb {
    fn drop(&mut self) {
        // Leave the adapter off the bus; otherwise it keeps ACKing after we exit.
        let _ = self.set_mode(MODE_RESET);
        let _ = self.handle.release_interface(0);
    }
}

fn packed_id(id: embedded_can::Id) -> u32 {
    canid::raw(id)
        | if canid::is_extended(id) {
            CAN_EFF_FLAG
        } else {
            0
        }
}

fn parse_host_frame(b: &[u8]) -> Option<Frame> {
    let echo_id = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let can_id = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    if can_id & CAN_ERR_FLAG != 0 {
        return None; // error frames are surfaced through the bus, not the RX table
    }
    let dlc = (b[8] as usize).min(8);
    let id = canid::make(can_id & 0x1FFF_FFFF, can_id & CAN_EFF_FLAG != 0)?;
    let data = if can_id & CAN_RTR_FLAG != 0 {
        &[][..]
    } else {
        &b[12..12 + dlc]
    };
    Some(Frame {
        id,
        data: Payload::new(data),
        echo: echo_id != ECHO_NONE,
    })
}

fn usb_err(e: rusb::Error) -> TransportError {
    match e {
        rusb::Error::Access => TransportError::Permission(PERMISSION_HINT.into()),
        rusb::Error::NoDevice => TransportError::Disconnected,
        rusb::Error::Busy => TransportError::Permission(
            "the device is claimed by another program (or by gs_usb.ko, use SocketCAN instead)"
                .into(),
        ),
        other => TransportError::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock every candleLight-class adapter runs at.
    const CANDLELIGHT: BtConst = BtConst {
        fclk_can: 48_000_000,
        tseg1_min: 1,
        tseg1_max: 16,
        tseg2_min: 1,
        tseg2_max: 8,
        sjw_max: 4,
        brp_min: 1,
        brp_max: 1024,
        brp_inc: 1,
    };

    #[test]
    fn solves_the_common_bitrates_exactly() {
        for bitrate in [125_000, 250_000, 500_000, 1_000_000] {
            let t = BitTiming::solve(&CANDLELIGHT, bitrate)
                .unwrap_or_else(|| panic!("no timing for {bitrate}"));
            let nbt = 1 + t.prop_seg + t.phase_seg1 + t.phase_seg2;
            let actual = CANDLELIGHT.fclk_can / (t.brp * nbt);
            assert_eq!(actual, bitrate, "{bitrate}: got {actual} from {t:?}");
            assert!(t.sjw >= 1 && t.sjw <= CANDLELIGHT.sjw_max);
            assert!(t.phase_seg2 >= CANDLELIGHT.tseg2_min);
        }
    }

    #[test]
    fn sample_point_lands_where_socketcan_puts_it() {
        // 500k wants 87.5%, 1M wants 75%.
        for (bitrate, want) in [(500_000u32, 875u32), (1_000_000, 750)] {
            let t = BitTiming::solve(&CANDLELIGHT, bitrate).unwrap();
            let nbt = 1 + t.prop_seg + t.phase_seg1 + t.phase_seg2;
            let sp = (1 + t.prop_seg + t.phase_seg1) * 1000 / nbt;
            assert!(
                sp.abs_diff(want) <= 40,
                "{bitrate}: sample point {sp}, wanted ~{want}"
            );
        }
    }

    #[test]
    fn refuses_a_bitrate_the_clock_cannot_reach() {
        assert!(BitTiming::solve(&CANDLELIGHT, 7_000_000).is_none());
        assert!(BitTiming::solve(&CANDLELIGHT, 0).is_none());
    }

    #[test]
    fn echo_frames_are_flagged_and_received_ones_are_not() {
        let mut b = [0u8; 20];
        b[0..4].copy_from_slice(&ECHO_NONE.to_le_bytes());
        b[4..8].copy_from_slice(&0x123u32.to_le_bytes());
        b[8] = 2;
        b[12] = 0xAB;
        b[13] = 0xCD;
        let f = parse_host_frame(&b).unwrap();
        assert!(!f.echo);
        assert_eq!(&*f.data, &[0xAB, 0xCD]);
        assert_eq!(canid::raw(f.id), 0x123);

        b[0..4].copy_from_slice(&7u32.to_le_bytes());
        assert!(parse_host_frame(&b).unwrap().echo);
    }

    #[test]
    fn extended_ids_round_trip_through_the_packed_form() {
        let id = canid::make(0x18FF_1234, true).unwrap();
        let packed = packed_id(id);
        assert_eq!(packed & CAN_EFF_FLAG, CAN_EFF_FLAG);
        let mut b = [0u8; 20];
        b[0..4].copy_from_slice(&ECHO_NONE.to_le_bytes());
        b[4..8].copy_from_slice(&packed.to_le_bytes());
        assert_eq!(parse_host_frame(&b).unwrap().id, id);
    }
}
