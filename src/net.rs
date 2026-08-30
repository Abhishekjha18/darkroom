//! Finding the address to print. Replaces `local-ip-address` / `if-addrs`.
//!
//! `std` has no interface enumeration at all, which turns out to be a gift.
//! This machine reports **seven non-loopback IPv4 interfaces and six of them
//! are `169.254.x.x` link-local junk** — so the obvious approach, "enumerate
//! and take the first non-loopback", prints a QR code that leads nowhere. And
//! it fails on camera, at the one beat of the demo that cannot be re-shot
//! casually. See BUILD.md §5.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// Addresses used only as a routing hint. `connect()` on UDP sets a peer in
/// the kernel without transmitting a packet, so nothing is ever sent to them
/// and no DNS lookup happens. Two, so a machine with an unusual route to one
/// still resolves.
const ROUTE_HINTS: [(Ipv4Addr, u16); 2] =
    [(Ipv4Addr::new(1, 1, 1, 1), 80), (Ipv4Addr::new(8, 8, 8, 8), 80)];

/// The IPv4 address of the interface carrying the default route.
pub fn local_ip() -> Option<Ipv4Addr> {
    for (host, port) in ROUTE_HINTS {
        let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { continue };
        if sock.connect((host, port)).is_err() {
            continue;
        }
        let Ok(addr) = sock.local_addr() else { continue };
        if let IpAddr::V4(ip) = addr.ip()
            && is_routable(ip)
        {
            return Some(ip);
        }
    }
    None
}

/// Rejects the three answers that are worse than no answer: `0.0.0.0`,
/// loopback, and link-local — a QR code pointing at any of them scans
/// perfectly and then fails to load.
fn is_routable(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_link_local() && !ip.is_broadcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_useless_addresses() {
        assert!(!is_routable(Ipv4Addr::new(0, 0, 0, 0)));
        assert!(!is_routable(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!is_routable(Ipv4Addr::new(169, 254, 13, 7)));
    }

    #[test]
    fn accepts_a_lan_address() {
        assert!(is_routable(Ipv4Addr::new(192, 168, 0, 105)));
        assert!(is_routable(Ipv4Addr::new(10, 0, 0, 4)));
    }

    /// Must not panic or hang on a machine with no network at all.
    #[test]
    fn survives_having_no_route() {
        let _ = local_ip();
    }
}
