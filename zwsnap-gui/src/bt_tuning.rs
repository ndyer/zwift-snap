//! Opt-in Bluetooth LE connection-interval re-tuning.
//!
//! A peripheral can advertise a deliberately slow "Peripheral Preferred
//! Connection Parameters" characteristic (0x2a04) purely to save battery — a
//! common case being heart-rate belts that cap their update rate at ~1 Hz.
//! BlueZ reads that characteristic during post-connect service discovery and,
//! ~2 s after the link is established, issues `MGMT_OP_LOAD_CONN_PARAM` to the
//! kernel, which then performs an LE Connection Update using the peripheral's
//! slow preference.
//!
//! When the privileged `bluetooth-control` plug is connected (never by
//! default), this module watches the kernel's management socket for freshly
//! connected LE devices and re-tightens the connection interval by sending its
//! own `MGMT_OP_LOAD_CONN_PARAM`. Because BlueZ's 0x2a04-driven update lands
//! after ours, the request is also re-sent on a short delay so it lands *after*
//! BlueZ and sticks.
//!
//! All of this is gated on `snapctl is-connected bluetooth-control`: without
//! that opt-in the module logs one line and does nothing, and any failure to
//! open the socket is logged and ignored so Zwift always still starts.

use std::io;
use std::os::fd::RawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cancel::Cancellation;
use crate::logging::Logger;

/// Values from <sys/socket.h> / <bluetooth/bluetooth.h> and
/// <bluetooth/hci.h> that `libc` does not expose; part of the stable Linux
/// Bluetooth ABI.
const AF_BLUETOOTH: i32 = 31;
const BTPROTO_HCI: i32 = 1;
const HCI_CHANNEL_CONTROL: u16 = 3;
/// Binding to the "none" device makes the control socket receive management
/// events for every controller, which is what we want for a system-wide watch.
const HCI_DEV_NONE: u16 = 0xFFFF;

/// Address type in the management protocol: 0 is BR/EDR, 1 is LE public and 2
/// is LE random. Only the LE values interest us.
const BDADDR_LE_PUBLIC: u8 = 0x01;
const BDADDR_LE_RANDOM: u8 = 0x02;

/// Subset of the kernel management protocol opcodes/events
/// (`include/net/bluetooth/mgmt.h`) needed by this module.
const MGMT_OP_LOAD_CONN_PARAM: u16 = 0x0035;
const MGMT_EV_DEVICE_CONNECTED: u16 = 0x000B;
const MGMT_EV_DEVICE_DISCONNECTED: u16 = 0x000C;

/// How long to wait for the next management event before looping. Kept short so
/// cancellation is noticed promptly and pending re-sends fire close to target.
const POLL_TIMEOUT_MS: i32 = 250;

/// Default target connection parameters.
///
/// `interval` is the min/max interval in 1.25 ms units, so 12 == 15 ms.
/// `timeout` is the supervision timeout in 10 ms units, so 200 == 2 s.
/// A 15 ms interval and zero latency give fast heart-rate updates at a modest
/// power cost; these are deliberately stricter than a belt's battery-saving
/// defaults, which is the entire point of the opt-in.
const DEFAULT_INTERVAL: u16 = 12;
const DEFAULT_TIMEOUT: u16 = 200;
const DEFAULT_RESEND_MS: u64 = 4000;

/// Connection parameters to apply, resolved once from the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Params {
    /// min/max connection interval in 1.25 ms units.
    interval: u16,
    /// supervision timeout in 10 ms units.
    timeout: u16,
    /// delay before the re-send that overrides BlueZ's 0x2a04 update.
    resend: Duration,
}

impl Params {
    /// Read the overrides `ZWIFT_BT_INTERVAL` (1.25 ms units), `ZWIFT_BT_TIMEOUT`
    /// (10 ms units) and `ZWIFT_BT_RESEND_MS` (milliseconds), falling back to the
    /// defaults above on absence or on unparseable values.
    fn from_env() -> Self {
        fn number(name: &str, default: u16) -> u16 {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        }
        let resend = std::env::var("ZWIFT_BT_RESEND_MS")
            .ok()
            .and_then(|value| value.parse().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_RESEND_MS));
        Params {
            interval: number("ZWIFT_BT_INTERVAL", DEFAULT_INTERVAL),
            timeout: number("ZWIFT_BT_TIMEOUT", DEFAULT_TIMEOUT),
            resend,
        }
    }

    fn interval_ms(self) -> u64 {
        // 1.25 ms == 5/4 ms, computed as integer arithmetic to stay exact.
        u64::from(self.interval) * 5 / 4
    }

    fn timeout_ms(self) -> u64 {
        u64::from(self.timeout) * 10
    }
}

/// A pending delayed re-send, keyed by the device it targets.
struct Pending {
    addr: [u8; 6],
    addr_type: u8,
    index: u16,
    deadline: Instant,
}

/// Closes a socket file descriptor when dropped, so the control socket is
/// released on every exit path of the watcher loop.
struct FdGuard(RawFd);

impl Drop for FdGuard {
    fn drop(&mut self) {
        // SAFETY: the guard owns the descriptor, closing it exactly once.
        unsafe { libc::close(self.0) };
    }
}

/// Report whether `bluetooth-control` is connected for this snap.
///
/// `snapctl is-connected` exits 0 only when the plug is connected. Anything
/// else (command missing, failure, non-zero exit) is treated as "not connected"
/// so the helper degrades to a no-op outside the snap or without the opt-in.
fn bluetooth_control_connected() -> bool {
    std::process::Command::new("snapctl")
        .args(["is-connected", "bluetooth-control"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Spawn the watcher on its own thread. It never panics and never blocks the
/// launcher: the channel carries failure messages back to the log.
pub fn spawn(logger: Arc<Logger>, cancellation: Cancellation) {
    std::thread::spawn(move || {
        if let Err(message) = watch(&logger, &cancellation) {
            logger.warning(&message);
        }
    });
}

/// Run the watcher, logging failures as a single human-readable string instead
/// of panicking so the launcher is never taken down by Bluetooth trouble.
fn watch(logger: &Logger, cancellation: &Cancellation) -> Result<(), String> {
    if !bluetooth_control_connected() {
        logger.info("bluetooth-control is not connected; leaving LE connection parameters alone");
        return Ok(());
    }
    let params = Params::from_env();
    let fd = open_control_socket()
        .map_err(|error| format!("unable to open the Bluetooth management socket: {error}"))?;
    let _fd = FdGuard(fd);

    let mut pending: Vec<Pending> = Vec::new();
    let mut buffer = [0_u8; 512];
    loop {
        cancellation
            .check()
            .map_err(|error| format!("Bluetooth tuning stopped: {error}"))?;
        fire_due_reesends(fd, logger, params, &mut pending);
        match poll_readable(fd, POLL_TIMEOUT_MS) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(error) if is_benign(&error) => continue,
            Err(error) => return Err(format!("Bluetooth management poll failed: {error}")),
        }
        match read_events(fd, &mut buffer) {
            Ok(n) => handle_events(&buffer[..n], fd, params, &mut pending, logger),
            Err(error) if is_benign(&error) => {}
            Err(error) => return Err(format!("Bluetooth management read failed: {error}")),
        }
    }
}

/// `poll`/`read` return `EINTR`/`EAGAIN` in benign situations (signal arrived,
/// another thread drained the event); those just mean "try again".
fn is_benign(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
    )
}

/// Open an `AF_BLUETOOTH`/`BTPROTO_HCI` control socket bound to all controllers.
///
/// `bluetooth-control` grants `CAP_NET_ADMIN`, which is required for the kernel
/// to treat this socket as trusted and to accept `MGMT_OP_LOAD_CONN_PARAM`.
fn open_control_socket() -> io::Result<RawFd> {
    // SAFETY: all arguments are plain integers; the flags request a raw,
    // close-on-exec, non-blocking Bluetooth HCI socket.
    let fd = unsafe {
        libc::socket(
            AF_BLUETOOTH,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            BTPROTO_HCI,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    #[repr(C)]
    struct SockaddrHci {
        family: libc::sa_family_t,
        dev: u16,
        channel: u16,
    }
    let address = SockaddrHci {
        family: AF_BLUETOOTH as libc::sa_family_t,
        dev: HCI_DEV_NONE,
        channel: HCI_CHANNEL_CONTROL,
    };
    // SAFETY: `address` outlives the call and `sizeof(SockaddrHci)` is the real
    // kernel ABI size (6 bytes) for `sockaddr_hci`.
    let result = unsafe {
        libc::bind(
            fd,
            &address as *const SockaddrHci as *const libc::sockaddr,
            std::mem::size_of::<SockaddrHci>() as libc::socklen_t,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        // SAFETY: fd is a valid socket descriptor we just created.
        unsafe { libc::close(fd) };
        return Err(error);
    }
    Ok(fd)
}

fn poll_readable(fd: RawFd, timeout_ms: i32) -> io::Result<bool> {
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pollfd points to one initialized pollfd for the duration of the call.
    let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
    if ready < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ready > 0 && (pollfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP)) != 0)
    }
}

/// Read one management event into `buffer`, returning how many bytes were read.
fn read_events(fd: RawFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: buffer is a valid, mutable byte slice held for the duration of the call.
    let received =
        unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, buffer.len()) };
    if received < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(received as usize)
    }
}

/// Parse the single management event in `buffer` (which holds one event plus
/// payload) and act on the events we care about.
fn handle_events(
    buffer: &[u8],
    fd: RawFd,
    params: Params,
    pending: &mut Vec<Pending>,
    logger: &Logger,
) {
    if buffer.len() < 6 {
        return;
    }
    let opcode = u16::from_le_bytes([buffer[0], buffer[1]]);
    let index = u16::from_le_bytes([buffer[2], buffer[3]]);
    let len = usize::from(u16::from_le_bytes([buffer[4], buffer[5]]));
    let payload = &buffer[6..(6 + len).min(buffer.len())];

    match opcode {
        MGMT_EV_DEVICE_CONNECTED => on_connected(payload, index, fd, params, pending, logger),
        MGMT_EV_DEVICE_DISCONNECTED => on_disconnected(payload, pending),
        _ => {}
    }
}

/// A device just connected: if it is LE, request the fast interval now and
/// schedule the delayed re-send that lands after BlueZ's 0x2a04 update.
fn on_connected(
    payload: &[u8],
    index: u16,
    fd: RawFd,
    params: Params,
    pending: &mut Vec<Pending>,
    logger: &Logger,
) {
    let Some((addr, addr_type)) = le_device_addr(payload) else {
        return;
    };

    // The immediate request gives a fast interval before BlueZ reads 0x2a04;
    // the delayed re-send below is the one that actually sticks.
    send_conn_param(fd, index, &addr, addr_type, params, logger, false);
    pending.push(Pending {
        addr,
        addr_type,
        index,
        deadline: Instant::now() + params.resend,
    });
    logger.info(&format!(
        "LE device {} connected; requesting connection interval {} ms (timeout {} ms)",
        format_bdaddr(&addr),
        params.interval_ms(),
        params.timeout_ms()
    ));
}

/// Extract the address and address type from a `MGMT_EV_DEVICE_CONNECTED`
/// payload, returning `None` for BR/EDR devices or malformed payloads.
///
/// The payload is addr(6) + type(1) + flags(4) + eir_len(2) + eir[]; only the
/// LE address types (public/random) are of interest here.
fn le_device_addr(payload: &[u8]) -> Option<([u8; 6], u8)> {
    if payload.len() < 13 {
        return None;
    }
    let addr_type = payload[6];
    if addr_type != BDADDR_LE_PUBLIC && addr_type != BDADDR_LE_RANDOM {
        return None;
    }
    let mut addr = [0_u8; 6];
    addr.copy_from_slice(&payload[0..6]);
    Some((addr, addr_type))
}

/// Drop any pending re-send for a device that has disconnected.
fn on_disconnected(payload: &[u8], pending: &mut Vec<Pending>) {
    if payload.len() < 7 {
        return;
    }
    let mut addr = [0_u8; 6];
    addr.copy_from_slice(&payload[0..6]);
    pending.retain(|entry| entry.addr != addr);
}

/// Send any pending re-sends whose deadline has passed.
fn fire_due_reesends(fd: RawFd, logger: &Logger, params: Params, pending: &mut Vec<Pending>) {
    let now = Instant::now();
    let mut i = 0;
    while i < pending.len() {
        if pending[i].deadline > now {
            i += 1;
            continue;
        }
        let entry = pending.swap_remove(i);
        send_conn_param(
            fd,
            entry.index,
            &entry.addr,
            entry.addr_type,
            params,
            logger,
            true,
        );
    }
}

/// Encode and write `MGMT_OP_LOAD_CONN_PARAM` for one device to `fd`.
///
/// A send failure on a device is not fatal: the override is a best-effort
/// tuning hint, so errors are logged and the watcher keeps running.
fn send_conn_param(
    fd: RawFd,
    index: u16,
    addr: &[u8; 6],
    addr_type: u8,
    params: Params,
    logger: &Logger,
    resend: bool,
) {
    let command = build_load_conn_param(index, addr, addr_type, params.interval, params.timeout);
    // SAFETY: fd is the control socket opened in `watch`, and `command` lives
    // for the duration of the write. `MSG_NOSIGNAL` avoids SIGPIPE if the
    // socket is closed under us.
    let written = unsafe {
        libc::send(
            fd,
            command.as_ptr() as *const libc::c_void,
            command.len(),
            libc::MSG_NOSIGNAL,
        )
    };
    if written < 0 {
        logger.debug(&format!(
            "connection-parameter request for {} failed: {}",
            format_bdaddr(addr),
            io::Error::last_os_error()
        ));
        return;
    }
    if resend {
        logger.info(&format!(
            "re-applying connection interval {} ms for {} after BlueZ's peripheral preference",
            params.interval_ms(),
            format_bdaddr(addr)
        ));
    }
}

/// Build the wire bytes of a `MGMT_OP_LOAD_CONN_PARAM` command for one device.
///
/// Layout (all little-endian, per `include/net/bluetooth/mgmt.h`):
/// * mgmt header: opcode(2) + index(2) + payload length(2).
/// * payload: param_count(2) then, per param, addr(6) + type(1) + min_interval(2)
///   + max_interval(2) + latency(2) + timeout(2).
fn build_load_conn_param(
    index: u16,
    addr: &[u8; 6],
    addr_type: u8,
    interval: u16,
    timeout: u16,
) -> Vec<u8> {
    let mut command = Vec::with_capacity(6 + 2 + 15);
    command.extend_from_slice(&MGMT_OP_LOAD_CONN_PARAM.to_le_bytes());
    command.extend_from_slice(&index.to_le_bytes());
    // payload length: param_count (2) + one conn_param (15).
    command.extend_from_slice(&(17_u16).to_le_bytes());
    command.extend_from_slice(&(1_u16).to_le_bytes());
    command.extend_from_slice(addr);
    command.push(addr_type);
    command.extend_from_slice(&interval.to_le_bytes()); // min_interval
    command.extend_from_slice(&interval.to_le_bytes()); // max_interval
    command.extend_from_slice(&(0_u16).to_le_bytes()); // latency
    command.extend_from_slice(&timeout.to_le_bytes());
    command
}

/// Format a little-endian on-the-wire Bluetooth address as `XX:XX:...`.
fn format_bdaddr(addr: &[u8; 6]) -> String {
    let mut octets = *addr;
    // The address arrives least-significant-octet first; reverse it for display.
    octets.reverse();
    octets
        .iter()
        .map(|octet| format!("{octet:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_conn_param_is_encoded_little_endian() {
        let addr = [0xD5, 0x1B, 0xE3, 0x52, 0x97, 0x04];
        let command = build_load_conn_param(0, &addr, BDADDR_LE_PUBLIC, 12, 200);
        assert_eq!(&command[0..6], &[0x35, 0x00, 0x00, 0x00, 0x11, 0x00]);
        // param_count = 1
        assert_eq!(&command[6..8], &[0x01, 0x00]);
        // address passthrough (wire order) then LE public type
        assert_eq!(&command[8..14], &addr);
        assert_eq!(command[14], BDADDR_LE_PUBLIC);
        // min=max=12, latency=0
        assert_eq!(&command[15..17], &[0x0C, 0x00]);
        assert_eq!(&command[17..19], &[0x0C, 0x00]);
        assert_eq!(&command[19..21], &[0x00, 0x00]);
        // timeout=200
        assert_eq!(&command[21..23], &[0xC8, 0x00]);
        assert_eq!(command.len(), 23);
    }

    #[test]
    fn address_formatting_reverses_wire_order() {
        assert_eq!(
            format_bdaddr(&[0xD5, 0x1B, 0xE3, 0x52, 0x97, 0x04]),
            "04:97:52:E3:1B:D5"
        );
    }

    #[test]
    fn interval_and_timeout_scale_correctly() {
        let params = Params {
            interval: 12,
            timeout: 200,
            resend: Duration::from_millis(4000),
        };
        assert_eq!(params.interval_ms(), 15);
        assert_eq!(params.timeout_ms(), 2000);
    }

    #[test]
    fn defaults_match_documented_values() {
        // Run in a fresh environment so no override leaks in from the test host.
        let params = Params::from_env();
        // Only assert the documented defaults when the host has not overridden.
        if std::env::var_os("ZWIFT_BT_INTERVAL").is_none() {
            assert_eq!(params.interval, 12);
        }
        if std::env::var_os("ZWIFT_BT_TIMEOUT").is_none() {
            assert_eq!(params.timeout, 200);
        }
        if std::env::var_os("ZWIFT_BT_RESEND_MS").is_none() {
            assert_eq!(params.resend, Duration::from_millis(4000));
        }
    }

    #[test]
    fn bredr_devices_are_ignored_and_le_parsed() {
        // addr + type=0 (BR/EDR) + flags + eir_len
        let mut bredr = vec![0; 13];
        bredr[0..6].copy_from_slice(&[0xD5, 0x1B, 0xE3, 0x52, 0x97, 0x04]);
        bredr[6] = 0x00;
        assert_eq!(le_device_addr(&bredr), None);

        // Same address with an LE public type is accepted and returned in
        // on-the-wire order.
        let mut le = bredr;
        le[6] = BDADDR_LE_PUBLIC;
        let (addr, ty) = le_device_addr(&le).unwrap();
        assert_eq!(addr, [0xD5, 0x1B, 0xE3, 0x52, 0x97, 0x04]);
        assert_eq!(ty, BDADDR_LE_PUBLIC);
    }

    #[test]
    fn truncated_connected_payload_is_ignored() {
        assert_eq!(le_device_addr(&[0; 12]), None);
    }
}
