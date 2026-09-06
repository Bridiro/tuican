//! What is plugged in right now. This is the module that replaces the Python
//! script's `--interface`/`--channel`/`--vid`/`--pid` flags with a list.

use super::gs_usb;
use super::spec::TransportSpec;

/// One row of the interface picker.
pub struct Candidate {
    pub spec: TransportSpec,
    /// Left column: what the thing is.
    pub label: String,
    /// Right column: where it is, or why it might not work.
    pub detail: String,
}

/// Everything we can see, best guess first.
pub fn scan(default_bitrate: u32) -> Vec<Candidate> {
    let mut out = Vec::new();
    #[cfg(target_os = "linux")]
    out.extend(socketcan_ifaces());
    out.extend(gs_usb_devices(default_bitrate));
    out.extend(serial_ports(default_bitrate));
    out.push(Candidate {
        spec: TransportSpec::Virtual,
        label: "virtual".into(),
        detail: "loopback — no hardware needed".into(),
    });
    out
}

fn gs_usb_devices(bitrate: u32) -> Vec<Candidate> {
    let Ok(devices) = rusb::devices() else {
        return Vec::new();
    };
    devices
        .iter()
        .filter_map(|d| {
            let desc = d.device_descriptor().ok()?;
            let (vid, pid) = (desc.vendor_id(), desc.product_id());
            gs_usb::KNOWN.iter().find(|(v, p, _)| *v == vid && *p == pid)?;
            Some(Candidate {
                spec: TransportSpec::GsUsb {
                    bus: d.bus_number(),
                    address: d.address(),
                    vid,
                    pid,
                    bitrate,
                },
                label: gs_usb::describe_ids(vid, pid),
                detail: format!(
                    "usb {:03}:{:03}  {vid:04x}:{pid:04x}",
                    d.bus_number(),
                    d.address()
                ),
            })
        })
        .collect()
}

fn serial_ports(bitrate: u32) -> Vec<Candidate> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };
    ports
        .into_iter()
        .filter(|p| {
            // Skip the Bluetooth and console pseudo-ports macOS always lists.
            let n = p.port_name.to_lowercase();
            !n.contains("bluetooth") && !n.contains("debug-console")
                && matches!(p.port_type, serialport::SerialPortType::UsbPort(_))
        })
        .map(|p| {
            let detail = match &p.port_type {
                serialport::SerialPortType::UsbPort(info) => info
                    .product
                    .clone()
                    .unwrap_or_else(|| format!("{:04x}:{:04x}", info.vid, info.pid)),
                _ => "serial".into(),
            };
            Candidate {
                spec: TransportSpec::Slcan { port: p.port_name.clone(), bitrate },
                label: format!("slcan {}", p.port_name),
                detail,
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn socketcan_ifaces() -> Vec<Candidate> {
    let Ok(entries) = std::fs::read_dir("/sys/class/net") else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("can") && !name.starts_with("vcan") {
                return None;
            }
            let state = super::socketcan::state_of(&name);
            let detail = match super::socketcan::bitrate_of(&name) {
                Some(b) => format!("{state}, {}", crate::fmt_bitrate(b)),
                None => format!("{state} — bring it up with `ip link`"),
            };
            Some(Candidate {
                spec: TransportSpec::SocketCan { iface: name.clone() },
                label: format!("socketcan {name}"),
                detail,
            })
        })
        .collect();
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

/// Printed by `--print-udev-rule`; the fix for the most common Linux failure.
pub const UDEV_RULE: &str = r#"# /etc/udev/rules.d/99-tuican.rules
# Lets a normal user talk to gs_usb / candleLight adapters.
SUBSYSTEM=="usb", ATTRS{idVendor}=="1d50", ATTRS{idProduct}=="606f", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="1209", ATTRS{idProduct}=="2323", MODE="0666", TAG+="uaccess"
SUBSYSTEM=="usb", ATTRS{idVendor}=="1cd2", ATTRS{idProduct}=="606f", MODE="0666", TAG+="uaccess"
# then: sudo udevadm control --reload-rules && sudo udevadm trigger
"#;
