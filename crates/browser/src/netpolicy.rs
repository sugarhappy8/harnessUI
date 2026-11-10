//! Where the agent's browser may go (ARCHITECTURE.md 9.3, 13.3 step 2).
//!
//! `http` and `https` only; loopback, private, link-local, carrier-grade NAT,
//! unique-local, unspecified, multicast and reserved addresses are blocked, judged on the
//! address the host name resolves to. These checks run on the tool's URL argument in
//! `describe` (a hard limit, no prompt) and again on every document request the browser
//! actually makes, each redirect hop included (`session.rs`).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use url::{Host, Url};

/// Schemes a page may be loaded from.
pub fn check_scheme(url: &Url) -> Result<(), String> {
    match url.scheme() {
        "http" | "https" => Ok(()),
        other => Err(format!(
            "{other}: URLs are blocked; the browser tool opens http and https pages only"
        )),
    }
}

/// Schemes a frame may show after it committed. `about:blank` and `about:srcdoc` carry no
/// network content; everything else that is not http(s) is blocked after the fact.
pub fn committed_scheme_allowed(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower == "about:blank"
        || lower.starts_with("about:blank#")
        || lower.starts_with("about:blank?")
        || lower == "about:srcdoc"
        || lower.starts_with("chrome-error://chromewebdata/")
        || lower.is_empty()
}

/// Address classes that are blocked by default. The ones marked `private` open up with
/// the "allow private networks" setting; the others never do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressClass {
    /// 127.0.0.0/8, ::1: the app's own llama-server lives here.
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

    /// Opened by the "allow private networks" setting.
    pub fn is_private_network(self) -> bool {
        matches!(
            self,
            Self::Private | Self::CarrierGradeNat | Self::UniqueLocal
        )
    }
}

/// The class of `ip` if it is one the browser must not reach by default; `None` for an
/// ordinary public address.
pub fn classify_ip(ip: IpAddr) -> Option<AddressClass> {
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
        return classify_v4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) embed an IPv4 destination.
    if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6].iter().all(|&s| s == 0) {
        return classify_v4(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            seg[6] as u8,
            (seg[7] >> 8) as u8,
            seg[7] as u8,
        ));
    }
    if seg[0] == 0x2002 {
        let embedded = Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        );
        if let Some(class) = classify_v4(embedded) {
            return Some(class);
        }
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

/// The address rules in force for one browser.
#[derive(Debug, Clone, Default)]
pub struct AddressPolicy {
    /// The "allow private networks" setting.
    pub allow_private: bool,
    /// Tests only: 127.0.0.1 is reachable on these ports.
    pub loopback_ports: Vec<u16>,
}

impl AddressPolicy {
    /// Refuses `ip` (reached on `port`) when its class is blocked.
    pub fn check_ip(&self, ip: IpAddr, port: u16) -> Result<(), String> {
        let Some(class) = classify_ip(ip) else {
            return Ok(());
        };
        if class.is_private_network() && self.allow_private {
            return Ok(());
        }
        if class == AddressClass::Loopback
            && ip == IpAddr::V4(Ipv4Addr::LOCALHOST)
            && self.loopback_ports.contains(&port)
        {
            return Ok(());
        }
        let hint = if class.is_private_network() {
            " Private networks can be allowed in Settings → Browser."
        } else {
            ""
        };
        Err(format!(
            "{ip} is {}, which the browser tool never opens.{hint}",
            class.describe()
        ))
    }
}

/// What a host name resolves to, cached briefly so a page with many frames does not
/// resolve the same host over and over.
#[derive(Debug, Default)]
pub struct DnsCache {
    entries: Mutex<HashMap<String, (Instant, Vec<IpAddr>)>>,
    /// Tests only: fixed answers for host names (see `BrowserConfig::test_dns_overrides`).
    #[cfg(feature = "test-hooks")]
    overrides: HashMap<String, IpAddr>,
}

/// How long a resolution is reused.
const DNS_TTL: Duration = Duration::from_secs(30);
/// How long a lookup may take.
const DNS_TIMEOUT: Duration = Duration::from_secs(8);

impl DnsCache {
    /// Tests only: a cache that answers `overrides` without asking DNS.
    #[cfg(feature = "test-hooks")]
    pub fn with_overrides(overrides: &[(String, IpAddr)]) -> Self {
        Self {
            entries: Mutex::default(),
            overrides: overrides
                .iter()
                .map(|(h, ip)| (h.to_ascii_lowercase(), *ip))
                .collect(),
        }
    }

    async fn lookup(&self, domain: &str, port: u16) -> Result<Vec<IpAddr>, String> {
        let key = domain.to_ascii_lowercase();
        #[cfg(feature = "test-hooks")]
        if let Some(ip) = self.overrides.get(&key) {
            return Ok(vec![*ip]);
        }
        if let Ok(entries) = self.entries.lock()
            && let Some((at, ips)) = entries.get(&key)
            && at.elapsed() < DNS_TTL
        {
            return Ok(ips.clone());
        }
        let resolved =
            tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((key.clone(), port)))
                .await
                .map_err(|_| format!("looking up {key} took too long"))?
                .map_err(|e| format!("{key} could not be resolved ({e})"))?;
        let mut ips: Vec<IpAddr> = resolved.map(|a| a.ip()).collect();
        ips.sort();
        ips.dedup();
        if ips.is_empty() {
            return Err(format!("{key} did not resolve to any address"));
        }
        if let Ok(mut entries) = self.entries.lock() {
            if entries.len() > 512 {
                entries.clear();
            }
            entries.insert(key, (Instant::now(), ips.clone()));
        }
        Ok(ips)
    }
}

// TODO(M6): the browser resolves the host again by itself. A name that answers with a
// public address here and a private one to the browser (DNS rebinding) is not caught;
// comparing Network.responseReceived's remoteIPAddress would at least refuse to read
// such a page.
/// Checks the scheme and every address `url`'s host resolves to. Returns the addresses.
///
/// A host that cannot be resolved is refused: without an address the class cannot be
/// judged. `localhost` and `*.localhost` are refused without a lookup, because Chrome
/// resolves them to loopback itself whatever DNS says.
pub async fn check_url(
    url: &Url,
    policy: &AddressPolicy,
    dns: &DnsCache,
) -> Result<Vec<IpAddr>, String> {
    check_scheme(url)?;
    let port = url.port_or_known_default().unwrap_or(80);
    let ips = match url.host() {
        None => return Err("the address has no host".into()),
        Some(Host::Ipv4(v4)) => vec![IpAddr::V4(v4)],
        Some(Host::Ipv6(v6)) => vec![IpAddr::V6(v6)],
        Some(Host::Domain(domain)) => {
            let lower = domain.trim_end_matches('.').to_ascii_lowercase();
            if lower == "localhost" || lower.ends_with(".localhost") {
                return Err(format!(
                    "{lower} is this computer (loopback), which the browser tool never opens."
                ));
            }
            dns.lookup(&lower, port).await?
        }
    };
    for ip in &ips {
        policy.check_ip(*ip, port)?;
    }
    Ok(ips)
}

/// The origin (`scheme://host[:port]`) the browser grant covers, punycode host.
pub fn origin_of(url: &Url) -> Option<String> {
    match url.scheme() {
        "http" | "https" => policy::sanitize::origin(url),
        _ => None,
    }
}

/// The origin of a URL string, or `"null"` for anything that has none.
pub fn origin_of_str(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|u| origin_of(&u))
        .unwrap_or_else(|| "null".into())
}

/// Parses what the model typed as a URL: adds `https://` to a bare host.
pub fn parse_user_url(input: &str) -> Result<Url, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("url is empty".into());
    }
    match Url::parse(trimmed) {
        Ok(url) => Ok(url),
        Err(url::ParseError::RelativeUrlWithoutBase) => Url::parse(&format!("https://{trimmed}"))
            .map_err(|e| format!("{trimmed} is not a valid address ({e})")),
        Err(e) => Err(format!("{trimmed} is not a valid address ({e})")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn schemes_other_than_http_and_https_are_blocked() {
        for ok in ["http://a.example/", "https://a.example/x"] {
            assert!(check_scheme(&Url::parse(ok).unwrap()).is_ok(), "{ok}");
        }
        for bad in [
            "file:///etc/passwd",
            "chrome://settings",
            "devtools://devtools/bundled/inspector.html",
            "data:text/html,hi",
            "javascript:alert(1)",
            "ftp://a.example/",
            "view-source:https://a.example/",
            "chrome-extension://abc/x.html",
            "blob:https://a.example/uuid",
        ] {
            let url = Url::parse(bad).unwrap();
            let err = check_scheme(&url).unwrap_err();
            assert!(err.contains("blocked"), "{bad}: {err}");
        }
    }

    #[test]
    fn committed_schemes() {
        assert!(committed_scheme_allowed("https://a.example/"));
        assert!(committed_scheme_allowed("about:blank"));
        assert!(committed_scheme_allowed("about:srcdoc"));
        assert!(!committed_scheme_allowed("file:///etc/passwd"));
        assert!(!committed_scheme_allowed("chrome://version"));
        assert!(!committed_scheme_allowed("data:text/html,x"));
        // Chrome's own error page for a load that failed: no remote content.
        assert!(committed_scheme_allowed("chrome-error://chromewebdata/"));
        assert!(!committed_scheme_allowed("chrome-error://other/"));
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
        for (ip, want) in cases {
            assert_eq!(classify_ip(v4(ip)), want, "{ip}");
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
            ("ff02::1", Some(AddressClass::Multicast)),
            ("::ffff:127.0.0.1", Some(AddressClass::Loopback)),
            ("::ffff:10.0.0.1", Some(AddressClass::Private)),
            ("::ffff:8.8.8.8", None),
            ("::127.0.0.1", Some(AddressClass::Loopback)),
            ("64:ff9b::a00:1", Some(AddressClass::Private)),
            ("2002:7f00:1::", Some(AddressClass::Loopback)),
            ("2001:db8::1", Some(AddressClass::Reserved)),
            ("2606:4700:4700::1111", None),
        ];
        for (ip, want) in cases {
            assert_eq!(classify_ip(ip.parse().unwrap()), want, "{ip}");
        }
    }

    #[test]
    fn the_private_network_switch_opens_only_private_classes() {
        let strict = AddressPolicy::default();
        let open = AddressPolicy {
            allow_private: true,
            ..Default::default()
        };
        for ip in ["10.0.0.1", "192.168.0.1", "100.64.1.1", "fd00::1"] {
            assert!(strict.check_ip(ip.parse().unwrap(), 80).is_err(), "{ip}");
            assert!(open.check_ip(ip.parse().unwrap(), 80).is_ok(), "{ip}");
        }
        for ip in [
            "127.0.0.1",
            "::1",
            "169.254.169.254",
            "0.0.0.0",
            "224.0.0.1",
        ] {
            assert!(open.check_ip(ip.parse().unwrap(), 80).is_err(), "{ip}");
        }
        assert!(open.check_ip(v4("1.1.1.1"), 443).is_ok());
    }

    #[test]
    fn test_loopback_ports_open_exactly_one_socket() {
        let p = AddressPolicy {
            allow_private: false,
            loopback_ports: vec![8123],
        };
        assert!(p.check_ip(v4("127.0.0.1"), 8123).is_ok());
        assert!(p.check_ip(v4("127.0.0.1"), 8124).is_err());
        assert!(p.check_ip(v4("127.0.0.2"), 8123).is_err());
        assert!(p.check_ip("::1".parse().unwrap(), 8123).is_err());
    }

    #[test]
    fn messages_say_why_and_what_to_do() {
        let err = AddressPolicy::default()
            .check_ip(v4("192.168.1.1"), 80)
            .unwrap_err();
        assert!(err.contains("private network"), "{err}");
        assert!(err.contains("Settings"), "{err}");
        let err = AddressPolicy::default()
            .check_ip(v4("127.0.0.1"), 80)
            .unwrap_err();
        assert!(err.contains("loopback"), "{err}");
        assert!(!err.contains("Settings"), "{err}");
    }

    #[tokio::test]
    async fn literal_and_localhost_hosts_are_judged_without_dns() {
        let dns = DnsCache::default();
        let policy = AddressPolicy::default();
        for bad in [
            "http://127.0.0.1:8080/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://[::1]/",
            "http://localhost/",
            "http://app.localhost./",
            "http://10.0.0.1/",
            "http://0.0.0.0/",
        ] {
            let url = Url::parse(bad).unwrap();
            assert!(check_url(&url, &policy, &dns).await.is_err(), "{bad}");
        }
        let ok = Url::parse("http://8.8.8.8/").unwrap();
        assert_eq!(
            check_url(&ok, &policy, &dns).await.unwrap(),
            vec![v4("8.8.8.8")]
        );
        let file = Url::parse("file:///etc/passwd").unwrap();
        assert!(
            check_url(&file, &policy, &dns)
                .await
                .unwrap_err()
                .contains("blocked")
        );
    }

    #[test]
    fn origins_are_punycode_and_drop_default_ports() {
        assert_eq!(
            origin_of(&Url::parse("https://Bücher.example:443/a?b").unwrap()).unwrap(),
            "https://xn--bcher-kva.example"
        );
        assert_eq!(
            origin_of_str("http://127.0.0.1:8080/x"),
            "http://127.0.0.1:8080"
        );
        assert_eq!(origin_of_str("about:blank"), "null");
        assert_eq!(origin_of_str("file:///x"), "null");
    }

    #[test]
    fn user_urls_get_https_when_bare() {
        assert_eq!(
            parse_user_url("example.com/path").unwrap().as_str(),
            "https://example.com/path"
        );
        assert_eq!(
            parse_user_url(" http://a.example ").unwrap().as_str(),
            "http://a.example/"
        );
        assert_eq!(parse_user_url("file:///x").unwrap().scheme(), "file");
        assert!(parse_user_url("").is_err());
    }
}
