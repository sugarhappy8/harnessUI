//! Which addresses a page may be fetched from (ARCHITECTURE.md 13.6).
//!
//! Loopback, private, link-local, carrier-grade NAT, unique-local, unspecified, multicast
//! and reserved addresses are refused, judged on the address a host name resolves to. There
//! is no setting that opens any of them: a search result or a redirect that points into
//! the user's own network is never followed.
//!
//! The classification is the browser tool's (`crates/browser/src/netpolicy.rs`), copied so
//! that this crate does not depend on the browser crate. A test in the app crate, which
//! depends on both, fails if the two ever disagree.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Address classes that are never fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    /// 127.0.0.0/8, ::1: this computer, where the app's own model server listens.
    Loopback,
    /// 10/8, 172.16/12, 192.168/16.
    Private,
    /// 169.254/16, fe80::/10 (also cloud metadata endpoints).
    LinkLocal,
    /// 100.64/10.
    CarrierGradeNat,
    /// fc00::/7 (and the deprecated fec0::/10 site-local block).
    UniqueLocal,
    /// 0.0.0.0/8, :: (on macOS and Linux 0.0.0.0 reaches local services).
    Unspecified,
    /// 224/4, ff00::/8, 255.255.255.255.
    Multicast,
    /// 240/4, 192.0.0/24, 198.18/15, documentation ranges.
    Reserved,
}

impl AddressClass {
    /// Plain-language name for messages.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Loopback => "a loopback (this computer) address",
            Self::Private => "a private network address",
            Self::LinkLocal => "a link-local address",
            Self::CarrierGradeNat => "a carrier-grade NAT (shared) address",
            Self::UniqueLocal => "a unique-local (private IPv6) address",
            Self::Unspecified => "an unspecified address (0.0.0.0 or ::)",
            Self::Multicast => "a multicast or broadcast address",
            Self::Reserved => "a reserved address",
        }
    }
}

/// The class of `ip` if it is one that is never fetched; `None` for an ordinary public
/// address.
pub fn classify(ip: IpAddr) -> Option<AddressClass> {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(ip: Ipv4Addr) -> Option<AddressClass> {
    let o = ip.octets();
    let class = match o {
        [0, ..] => AddressClass::Unspecified,
        [127, ..] => AddressClass::Loopback,
        [10, ..] => AddressClass::Private,
        [172, b, ..] if (16..=31).contains(&b) => AddressClass::Private,
        [192, 168, ..] => AddressClass::Private,
        [169, 254, ..] => AddressClass::LinkLocal,
        [100, b, ..] if (64..=127).contains(&b) => AddressClass::CarrierGradeNat,
        [255, 255, 255, 255] => AddressClass::Multicast,
        [a, ..] if (224..=239).contains(&a) => AddressClass::Multicast,
        [a, ..] if a >= 240 => AddressClass::Reserved,
        [192, 0, 0, _] => AddressClass::Reserved,
        [192, 0, 2, _] | [198, 51, 100, _] | [203, 0, 113, _] => AddressClass::Reserved,
        [198, b, ..] if b == 18 || b == 19 => AddressClass::Reserved,
        _ => return None,
    };
    Some(class)
}

fn embedded_v4(high: u16, low: u16) -> Ipv4Addr {
    Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
}

fn classify_v6(ip: Ipv6Addr) -> Option<AddressClass> {
    if ip.is_unspecified() {
        return Some(AddressClass::Unspecified);
    }
    if ip.is_loopback() {
        return Some(AddressClass::Loopback);
    }
    // IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible (::a.b.c.d) forms carry an IPv4
    // address that the OS may connect to as such.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    let seg = ip.segments();
    if seg[..6].iter().all(|&s| s == 0) {
        return classify_v4(embedded_v4(seg[6], seg[7]));
    }
    // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) embed an IPv4 destination.
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6].iter().all(|&s| s == 0) {
        return classify_v4(embedded_v4(seg[6], seg[7]));
    }
    if seg[0] == 0x2002
        && let Some(class) = classify_v4(embedded_v4(seg[1], seg[2]))
    {
        return Some(class);
    }
    match seg[0] {
        s if s & 0xfe00 == 0xfc00 => Some(AddressClass::UniqueLocal),
        s if s & 0xffc0 == 0xfe80 => Some(AddressClass::LinkLocal),
        s if s & 0xffc0 == 0xfec0 => Some(AddressClass::UniqueLocal),
        s if s & 0xff00 == 0xff00 => Some(AddressClass::Multicast),
        // Documentation prefix 2001:db8::/32.
        0x2001 if seg[1] == 0x0db8 => Some(AddressClass::Reserved),
        _ => None,
    }
}

/// An address that was refused, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocked {
    pub ip: IpAddr,
    pub class: AddressClass,
}

/// The address rules of one fetcher. There is nothing to configure: every class above is
/// refused. Tests can open single loopback ports.
#[derive(Debug, Clone, Default)]
pub struct Guard {
    /// Tests only: 127.0.0.1 is reachable on these ports.
    #[cfg(feature = "test-hooks")]
    loopback_ports: Vec<u16>,
}

impl Guard {
    /// The guard the app uses: nothing is open.
    pub fn new() -> Self {
        Self::default()
    }

    /// Tests only: a guard that lets 127.0.0.1 through on exactly these ports.
    #[cfg(feature = "test-hooks")]
    pub fn allowing_loopback_ports(ports: &[u16]) -> Self {
        Self {
            loopback_ports: ports.to_vec(),
        }
    }

    /// Refuses `ip` (reached on `port`) when its class is one that is never fetched.
    pub fn check(&self, ip: IpAddr, port: u16) -> Result<(), Blocked> {
        let Some(class) = classify(ip) else {
            return Ok(());
        };
        #[cfg(feature = "test-hooks")]
        if ip == IpAddr::V4(Ipv4Addr::LOCALHOST) && self.loopback_ports.contains(&port) {
            return Ok(());
        }
        let _ = port;
        Err(Blocked { ip, class })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ipv4_classes() {
        let cases = [
            ("127.0.0.1", Some(AddressClass::Loopback)),
            ("127.255.0.9", Some(AddressClass::Loopback)),
            ("10.1.2.3", Some(AddressClass::Private)),
            ("172.16.0.1", Some(AddressClass::Private)),
            ("172.31.255.255", Some(AddressClass::Private)),
            ("172.32.0.1", None),
            ("172.15.0.1", None),
            ("192.168.1.1", Some(AddressClass::Private)),
            ("169.254.169.254", Some(AddressClass::LinkLocal)),
            ("100.64.0.1", Some(AddressClass::CarrierGradeNat)),
            ("100.127.255.255", Some(AddressClass::CarrierGradeNat)),
            ("100.128.0.1", None),
            ("0.0.0.0", Some(AddressClass::Unspecified)),
            ("0.1.2.3", Some(AddressClass::Unspecified)),
            ("224.0.0.1", Some(AddressClass::Multicast)),
            ("255.255.255.255", Some(AddressClass::Multicast)),
            ("240.0.0.1", Some(AddressClass::Reserved)),
            ("198.18.0.1", Some(AddressClass::Reserved)),
            ("192.0.2.1", Some(AddressClass::Reserved)),
            ("8.8.8.8", None),
            ("93.184.216.34", None),
        ];
        for (addr, want) in cases {
            assert_eq!(classify(ip(addr)), want, "{addr}");
        }
    }

    #[test]
    fn ipv6_classes_including_embedded_ipv4() {
        let cases = [
            ("::1", Some(AddressClass::Loopback)),
            ("::", Some(AddressClass::Unspecified)),
            ("fc00::1", Some(AddressClass::UniqueLocal)),
            ("fd12:3456::1", Some(AddressClass::UniqueLocal)),
            ("fe80::1", Some(AddressClass::LinkLocal)),
            ("fec0::1", Some(AddressClass::UniqueLocal)),
            ("ff02::1", Some(AddressClass::Multicast)),
            ("::ffff:127.0.0.1", Some(AddressClass::Loopback)),
            ("::ffff:10.0.0.1", Some(AddressClass::Private)),
            ("::ffff:169.254.169.254", Some(AddressClass::LinkLocal)),
            ("::ffff:100.64.0.1", Some(AddressClass::CarrierGradeNat)),
            ("::ffff:0.0.0.0", Some(AddressClass::Unspecified)),
            ("::ffff:8.8.8.8", None),
            ("::127.0.0.1", Some(AddressClass::Loopback)),
            ("64:ff9b::a00:1", Some(AddressClass::Private)),
            ("64:ff9b::7f00:1", Some(AddressClass::Loopback)),
            ("2002:7f00:1::", Some(AddressClass::Loopback)),
            ("2002:c0a8:101::", Some(AddressClass::Private)),
            ("2001:db8::1", Some(AddressClass::Reserved)),
            ("2606:4700:4700::1111", None),
        ];
        for (addr, want) in cases {
            assert_eq!(classify(ip(addr)), want, "{addr}");
        }
    }

    /// Every class is refused, on every port, and nothing can switch that off.
    #[test]
    fn the_guard_refuses_every_class() {
        let guard = Guard::new();
        let blocked = [
            ("127.0.0.1", AddressClass::Loopback),
            ("::1", AddressClass::Loopback),
            ("10.0.0.1", AddressClass::Private),
            ("192.168.0.1", AddressClass::Private),
            ("172.20.1.1", AddressClass::Private),
            ("fd00::1", AddressClass::UniqueLocal),
            ("169.254.169.254", AddressClass::LinkLocal),
            ("fe80::1", AddressClass::LinkLocal),
            ("100.64.1.1", AddressClass::CarrierGradeNat),
            ("0.0.0.0", AddressClass::Unspecified),
            ("::", AddressClass::Unspecified),
            ("224.0.0.1", AddressClass::Multicast),
            ("240.0.0.1", AddressClass::Reserved),
        ];
        for (addr, class) in blocked {
            for port in [80, 443, 8080, 11434] {
                assert_eq!(
                    guard.check(ip(addr), port),
                    Err(Blocked {
                        ip: ip(addr),
                        class
                    }),
                    "{addr}:{port}"
                );
            }
        }
        for addr in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert_eq!(guard.check(ip(addr), 443), Ok(()), "{addr}");
        }
    }

    #[test]
    fn messages_name_the_class() {
        assert!(AddressClass::Loopback.describe().contains("this computer"));
        assert!(AddressClass::Private.describe().contains("private network"));
        assert!(AddressClass::LinkLocal.describe().contains("link-local"));
        assert!(AddressClass::CarrierGradeNat.describe().contains("shared"));
        assert!(AddressClass::Unspecified.describe().contains("0.0.0.0"));
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn test_loopback_ports_open_exactly_one_socket() {
        let guard = Guard::allowing_loopback_ports(&[8123]);
        assert!(guard.check(ip("127.0.0.1"), 8123).is_ok());
        assert!(guard.check(ip("127.0.0.1"), 8124).is_err());
        assert!(guard.check(ip("127.0.0.2"), 8123).is_err());
        assert!(guard.check(ip("::1"), 8123).is_err());
        assert!(guard.check(ip("10.0.0.1"), 8123).is_err());
    }
}
