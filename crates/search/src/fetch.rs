//! The guarded fetcher: the only way this crate reads a page (ARCHITECTURE.md 13.6).
//!
//! A search result is an address someone else chose, and so is every redirect. Left
//! unchecked, either could point the app at the user's own computer or network: the model
//! server on loopback, a router, a cloud metadata service. So for every request, the first
//! one and each redirect hop:
//!
//! 1. Only `http` and `https`, and no user name or password in the address.
//! 2. The host name is resolved here, once. Every address it resolves to is checked
//!    ([`crate::netguard`]): loopback, private, link-local, shared (CGNAT), unique-local,
//!    unspecified, multicast and reserved addresses are refused. `localhost` names are
//!    refused without a lookup.
//! 3. The connection goes to exactly the addresses that were checked. The HTTP client of
//!    the request has no other resolver ([`Pinned`]) and uses no proxy, so there is no
//!    second lookup that could give a different answer (DNS rebinding).
//! 4. Redirects are not followed by the HTTP client. A redirect comes back as a response;
//!    its target goes through steps 1 to 3 again. At most [`MAX_REDIRECTS`].
//!
//! Besides that: a time limit for the whole fetch, a size limit for the body, only the
//! content types asked for, no cookies, no referrer, and a `User-Agent` that names the app
//! and its version and nothing else.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

use crate::netguard::{AddressClass, Guard};

/// How long one fetch may take, redirects and body included.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// How much of a body is read.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Redirects followed at most.
pub const MAX_REDIRECTS: usize = 5;
/// How long resolving a host name may take.
const DNS_TIMEOUT: Duration = Duration::from_secs(5);

/// What kind of answer a request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accept {
    /// A page to read: `text/html` or `text/plain`.
    Page,
    /// A package index's answer: JSON.
    Json,
}

/// A fetched body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The address the body came from, after redirects.
    pub url: Url,
    /// The media type without parameters, lower case: `text/html`.
    pub content_type: String,
    /// The body as text.
    pub text: String,
    /// The body was longer than the limit; this is its start.
    pub truncated: bool,
    /// How many redirects were followed.
    pub redirects: usize,
}

/// Why a fetch was refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("only http and https addresses are read, not {0}:")]
    Scheme(String),
    #[error("the address carries a user name or password")]
    Credentials,
    #[error("the address has no host")]
    NoHost,
    #[error("{host} is {} ({ip}), and pages are never read from there", class.describe())]
    Blocked {
        host: String,
        ip: IpAddr,
        class: AddressClass,
    },
    #[error("{0} is this computer, and pages are never read from there")]
    LocalName(String),
    #[error("the site's address could not be found ({0})")]
    Dns(String),
    #[error("the site did not answer within {0} seconds")]
    Timeout(u64),
    #[error("the site redirected more than {MAX_REDIRECTS} times")]
    TooManyRedirects,
    #[error("the site redirected to an address that cannot be read ({0})")]
    BadRedirect(String),
    #[error("the site answered with an error ({0})")]
    Status(u16),
    #[error("it is not a web page or plain text ({0})")]
    ContentType(String),
    #[error("the site sent the page in a packed form this app does not read ({0})")]
    Encoding(String),
    #[error("the connection failed ({0})")]
    Network(String),
    #[error("the site asked for a check that only a web browser can pass")]
    Challenge,
    #[error("the page has no text to read (it may need scripts to show anything)")]
    NoText,
}

impl FetchError {
    /// Whether the address rules refused the request (as opposed to the site failing).
    pub fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::Scheme(_)
                | Self::Credentials
                | Self::NoHost
                | Self::Blocked { .. }
                | Self::LocalName(_)
        )
    }
}

/// The resolver of one request: it knows the addresses that were checked for one host
/// name, and nothing else. A lookup of any other name fails, so the HTTP client can never
/// ask the system resolver.
struct Pinned {
    host: String,
    addrs: Vec<SocketAddr>,
}

impl Resolve for Pinned {
    fn resolve(&self, name: Name) -> Resolving {
        let asked = name.as_str().trim_end_matches('.');
        let answer: Result<Addrs, Box<dyn std::error::Error + Send + Sync>> =
            if asked.eq_ignore_ascii_case(&self.host) {
                Ok(Box::new(self.addrs.clone().into_iter()))
            } else {
                Err(format!("{asked} is not the host that was checked").into())
            };
        Box::pin(std::future::ready(answer))
    }
}

/// A host name as it is compared and resolved: lower case, without the dot at its end.
fn plain_host(domain: &str) -> String {
    domain.trim_end_matches('.').to_ascii_lowercase()
}

/// One answer of one hop.
enum Hop {
    Done(Fetched),
    Redirect(Url),
}

/// Reads pages under the address rules.
#[derive(Debug, Clone)]
pub struct Fetcher {
    user_agent: String,
    timeout: Duration,
    max_bytes: usize,
    guard: Guard,
    /// Tests only: fixed addresses for host names.
    #[cfg(feature = "test-hooks")]
    test_dns: Vec<(String, IpAddr)>,
}

impl Fetcher {
    /// A fetcher that introduces itself as `user_agent` (`<app name>/<version>`).
    pub fn new(user_agent: impl Into<String>) -> Self {
        Self {
            user_agent: user_agent.into(),
            timeout: DEFAULT_TIMEOUT,
            max_bytes: MAX_BODY_BYTES,
            guard: Guard::new(),
            #[cfg(feature = "test-hooks")]
            test_dns: Vec::new(),
        }
    }

    /// Another time limit for one fetch.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Another size limit for a body.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Tests only: 127.0.0.1 is reachable on exactly these ports.
    #[cfg(feature = "test-hooks")]
    #[must_use]
    pub fn with_test_loopback_ports(mut self, ports: &[u16]) -> Self {
        self.guard = Guard::allowing_loopback_ports(ports);
        self
    }

    /// Tests only: these host names resolve to these addresses (a name may be given more
    /// than once) instead of being looked up.
    #[cfg(feature = "test-hooks")]
    #[must_use]
    pub fn with_test_dns(mut self, entries: &[(&str, IpAddr)]) -> Self {
        self.test_dns = entries
            .iter()
            .map(|(host, ip)| (host.to_ascii_lowercase(), *ip))
            .collect();
        self
    }

    /// The addresses of a host name: the test answers when there are any, else a lookup.
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<IpAddr>, FetchError> {
        #[cfg(feature = "test-hooks")]
        {
            let fixed: Vec<IpAddr> = self
                .test_dns
                .iter()
                .filter(|(name, _)| name == host)
                .map(|(_, ip)| *ip)
                .collect();
            if !fixed.is_empty() {
                return Ok(fixed);
            }
        }
        let lookup = tokio::net::lookup_host((host.to_string(), port));
        let found = tokio::time::timeout(DNS_TIMEOUT, lookup)
            .await
            .map_err(|_| FetchError::Dns(host.to_string()))?
            .map_err(|_| FetchError::Dns(host.to_string()))?;
        let mut addresses: Vec<IpAddr> = found.map(|a| a.ip()).collect();
        addresses.sort();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(FetchError::Dns(host.to_string()));
        }
        Ok(addresses)
    }

    /// Steps 1 and 2 for one address: the scheme, then every address the host resolves
    /// to. Returns the checked addresses; nothing is connected to.
    pub async fn check(&self, url: &Url) -> Result<Vec<SocketAddr>, FetchError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(FetchError::Scheme(url.scheme().to_string()));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(FetchError::Credentials);
        }
        let port = url.port_or_known_default().ok_or(FetchError::NoHost)?;
        let (shown, addresses) = match url.host() {
            None => return Err(FetchError::NoHost),
            Some(url::Host::Ipv4(ip)) => (ip.to_string(), vec![IpAddr::V4(ip)]),
            Some(url::Host::Ipv6(ip)) => (ip.to_string(), vec![IpAddr::V6(ip)]),
            Some(url::Host::Domain(domain)) => {
                let host = plain_host(domain);
                if host.is_empty() {
                    return Err(FetchError::NoHost);
                }
                // Resolved to loopback by every system, whatever a name server says.
                if host == "localhost" || host.ends_with(".localhost") {
                    return Err(FetchError::LocalName(host));
                }
                let addresses = self.resolve(&host, port).await?;
                (host, addresses)
            }
        };
        // One refused address refuses the name: a name with a public and a private answer
        // must not reach the private one.
        for ip in &addresses {
            self.guard
                .check(*ip, port)
                .map_err(|blocked| FetchError::Blocked {
                    host: shown.clone(),
                    ip: blocked.ip,
                    class: blocked.class,
                })?;
        }
        Ok(addresses
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect())
    }

    /// The HTTP client of one hop: it can reach the checked addresses of this one host and
    /// nothing else, follows no redirect, uses no proxy and keeps nothing between requests.
    fn client_for(&self, url: &Url, checked: &[SocketAddr]) -> Result<reqwest::Client, FetchError> {
        let host = match url.host() {
            Some(url::Host::Domain(domain)) => plain_host(domain),
            // A literal address is connected to as written; no name is ever resolved.
            _ => String::new(),
        };
        reqwest::Client::builder()
            .user_agent(self.user_agent.as_str())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(Pinned {
                host,
                addrs: checked.to_vec(),
            }))
            .referer(false)
            .connect_timeout(self.timeout)
            .timeout(self.timeout)
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| FetchError::Network(e.without_url().to_string()))
    }

    /// One request: check, connect to what was checked, read the answer or the redirect.
    async fn hop(&self, url: &Url, accept: Accept, redirects: usize) -> Result<Hop, FetchError> {
        let checked = self.check(url).await?;
        let client = self.client_for(url, &checked)?;
        let wanted = match accept {
            Accept::Page => "text/html, text/plain;q=0.9",
            Accept::Json => "application/json",
        };
        let failed = |error: reqwest::Error| self.failure(error);
        let mut response = client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, wanted)
            // The body is read as it is sent; nothing here unpacks one.
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(failed)?;
        let status = response.status().as_u16();
        let header = |name: reqwest::header::HeaderName| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .trim()
                .to_string()
        };
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = header(reqwest::header::LOCATION);
            return Ok(Hop::Redirect(redirect_target(url, &location)?));
        }
        if !(200..300).contains(&status) {
            return Err(FetchError::Status(status));
        }
        let (media, charset) = media_type(&header(reqwest::header::CONTENT_TYPE));
        if !accepts(accept, &media) {
            return Err(FetchError::ContentType(media));
        }
        let packed = header(reqwest::header::CONTENT_ENCODING).to_ascii_lowercase();
        if !packed.is_empty() && packed != "identity" {
            return Err(FetchError::Encoding(packed));
        }
        let mut body: Vec<u8> = Vec::new();
        let mut truncated = false;
        while let Some(chunk) = response.chunk().await.map_err(failed)? {
            let room = self.max_bytes - body.len();
            if chunk.len() > room {
                body.extend_from_slice(&chunk[..room]);
                truncated = true;
                break;
            }
            body.extend_from_slice(&chunk);
            if body.len() == self.max_bytes {
                // Whether more would follow is not worth waiting for.
                truncated = response
                    .content_length()
                    .is_none_or(|n| n > body.len() as u64);
                break;
            }
        }
        Ok(Hop::Done(Fetched {
            url: url.clone(),
            text: decode(&body, charset.as_deref(), media == "text/html"),
            content_type: media,
            truncated,
            redirects,
        }))
    }

    /// Fetches `url`, following redirects by hand under the same rules.
    pub async fn get(&self, url: &Url, accept: Accept) -> Result<Fetched, FetchError> {
        let all_hops = async {
            let mut current = url.clone();
            let mut redirects = 0;
            loop {
                match self.hop(&current, accept, redirects).await? {
                    Hop::Done(fetched) => return Ok(fetched),
                    Hop::Redirect(_) if redirects == MAX_REDIRECTS => {
                        return Err(FetchError::TooManyRedirects);
                    }
                    Hop::Redirect(next) => {
                        redirects += 1;
                        current = next;
                    }
                }
            }
        };
        match tokio::time::timeout(self.timeout, all_hops).await {
            Ok(result) => result.map_err(|error| match error {
                // The client's own limit is the same one; it reads as a network failure.
                FetchError::Network(detail) if detail.contains("timed out") => self.timed_out(),
                other => other,
            }),
            Err(_) => Err(self.timed_out()),
        }
    }

    fn timed_out(&self) -> FetchError {
        FetchError::Timeout(self.timeout.as_secs().max(1))
    }

    /// What a failed request is reported as. A request that ran out of time is a timeout
    /// whichever of the two clocks noticed first (the HTTP client's, or the one around
    /// the whole fetch in [`Fetcher::get`]), and it is recognised by the error's kind: its
    /// words do not say "timed out" on every system.
    fn failure(&self, error: reqwest::Error) -> FetchError {
        if error.is_timeout() {
            self.timed_out()
        } else {
            FetchError::Network(error.without_url().to_string())
        }
    }
}

/// The media type and the charset of a `Content-Type` header value: `text/html` and
/// `utf-8` for `Text/HTML; charset="UTF-8"`.
pub fn media_type(header: &str) -> (String, Option<String>) {
    let mut parts = header.split(';');
    let media = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let charset = parts.find_map(|part| {
        let (name, value) = part.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
            .filter(|value| !value.is_empty())
    });
    (media, charset)
}

/// Whether a media type is one a request of this kind takes.
pub fn accepts(accept: Accept, media_type: &str) -> bool {
    match accept {
        Accept::Page => matches!(media_type, "text/html" | "text/plain"),
        Accept::Json => {
            media_type == "application/json"
                || (media_type.starts_with("application/") && media_type.ends_with("+json"))
        }
    }
}

/// The charset an HTML page names in its first bytes: `<meta charset="…">` or the older
/// `<meta http-equiv="Content-Type" content="…; charset=…">`.
fn charset_in_markup(body: &[u8]) -> Option<String> {
    let head = &body[..body.len().min(2048)];
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = head[from..].find("charset") {
        let start = from + at;
        from = start + "charset".len();
        // Only inside a meta tag: the last "<" before it has to open one.
        let tag = head[..start].rfind('<').map(|open| &head[open..start]);
        if !tag.is_some_and(|t| t.starts_with("<meta")) {
            continue;
        }
        let rest = head[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let name: String = rest
            .trim_start()
            .trim_start_matches(['"', '\''])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
            .collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// A body as text. The charset the server named is used; an HTML page that names none may
/// name it in its first bytes (`<meta charset>`); otherwise it is read as UTF-8, with
/// anything that is not UTF-8 replaced.
pub fn decode(body: &[u8], charset: Option<&str>, html: bool) -> String {
    let named = charset
        .map(str::to_string)
        .or_else(|| html.then(|| charset_in_markup(body)).flatten());
    let encoding = named
        .and_then(|name| encoding_rs::Encoding::for_label(name.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    // Removes a byte-order mark, and follows it when it names another encoding.
    let (text, _, _) = encoding.decode(body);
    text.into_owned()
}

/// Where a redirect leads: `location` read against the address that sent it. Only `http`
/// and `https` targets without a user name or password are returned.
pub fn redirect_target(from: &Url, location: &str) -> Result<Url, FetchError> {
    let location = location.trim();
    if location.is_empty() {
        return Err(FetchError::BadRedirect("no address".into()));
    }
    let shown: String = location.chars().take(120).collect();
    let target = from
        .join(location)
        .map_err(|_| FetchError::BadRedirect(shown))?;
    if !matches!(target.scheme(), "http" | "https") {
        return Err(FetchError::Scheme(target.scheme().to_string()));
    }
    if !target.username().is_empty() || target.password().is_some() {
        return Err(FetchError::Credentials);
    }
    if target.host_str().is_none() {
        return Err(FetchError::NoHost);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On macOS the HTTP client's own clock noticed a slow site before the clock around
    /// the whole fetch did, and its error reads "error sending request" with no word
    /// about time: the fetch was reported as a failed connection. This makes that clock
    /// the one that fires, on every system, and checks what the error is reported as.
    #[tokio::test]
    async fn a_request_that_runs_out_of_time_is_a_timeout_whatever_its_words() {
        // A site that takes the connection and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                held.push(stream);
            }
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(150))
            .build()
            .expect("client");
        let error = client
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .expect_err("no answer");
        let fetcher = Fetcher::new("test").with_timeout(Duration::from_secs(7));
        assert_eq!(fetcher.failure(error), FetchError::Timeout(7));

        // A connection that is refused is not a timeout.
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let error = client
            .get(format!("http://127.0.0.1:{closed}/"))
            .send()
            .await
            .expect_err("nothing listens");
        assert!(matches!(fetcher.failure(error), FetchError::Network(_)));
    }

    #[test]
    fn media_types_are_read_from_the_header() {
        assert_eq!(media_type("text/html"), ("text/html".into(), None));
        assert_eq!(
            media_type("Text/HTML; charset=\"UTF-8\""),
            ("text/html".into(), Some("utf-8".into()))
        );
        assert_eq!(
            media_type("text/plain;charset=ISO-8859-1; format=flowed"),
            ("text/plain".into(), Some("iso-8859-1".into()))
        );
        assert_eq!(
            media_type(" application/vnd.npm.install-v1+json ; q=1"),
            ("application/vnd.npm.install-v1+json".into(), None)
        );
        assert_eq!(media_type(""), (String::new(), None));
    }

    #[test]
    fn pages_are_html_or_plain_text_and_nothing_else() {
        for ok in ["text/html", "text/plain"] {
            assert!(accepts(Accept::Page, ok), "{ok}");
        }
        for no in [
            "application/pdf",
            "application/json",
            "application/xhtml+xml",
            "application/octet-stream",
            "image/png",
            "image/svg+xml",
            "text/javascript",
            "text/css",
            "text/xml",
            "video/mp4",
            "application/zip",
            "",
        ] {
            assert!(!accepts(Accept::Page, no), "{no}");
        }
        for ok in ["application/json", "application/vnd.npm.install-v1+json"] {
            assert!(accepts(Accept::Json, ok), "{ok}");
        }
        for no in ["text/html", "text/plain", "application/javascript", ""] {
            assert!(!accepts(Accept::Json, no), "{no}");
        }
    }

    #[test]
    fn bodies_are_decoded_by_their_charset() {
        assert_eq!(decode("héllo".as_bytes(), None, true), "héllo");
        assert_eq!(decode(b"h\xE9llo", Some("iso-8859-1"), true), "héllo");
        assert_eq!(decode(b"h\xE9llo", Some("windows-1252"), false), "héllo");
        // No charset named anywhere: UTF-8, with what is not UTF-8 replaced.
        assert_eq!(decode(b"h\xE9llo", None, false), "h\u{FFFD}llo");
        // A page that names its charset itself.
        assert_eq!(
            decode(
                b"<html><head><meta charset=\"iso-8859-1\"><title>caf\xE9</title>",
                None,
                true
            ),
            "<html><head><meta charset=\"iso-8859-1\"><title>café</title>"
        );
        assert_eq!(
            decode(
                b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\"><p>\x93x\x94</p>",
                None,
                true
            ),
            "<meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1252\"><p>\u{201C}x\u{201D}</p>"
        );
        // Plain text is never sniffed, an unknown name falls back to UTF-8, and a
        // byte-order mark is not part of the text.
        assert_eq!(
            decode(b"charset=iso-8859-1 caf\xE9", None, false),
            "charset=iso-8859-1 caf\u{FFFD}"
        );
        assert_eq!(
            decode("ok é".as_bytes(), Some("no-such-charset"), true),
            "ok é"
        );
        assert_eq!(decode(b"\xEF\xBB\xBFtext", None, false), "text");
        assert_eq!(decode(b"", None, true), "");
    }

    #[test]
    fn redirect_targets() {
        let from = Url::parse("https://a.example/docs/page?x=1").unwrap();
        let to = |location: &str| redirect_target(&from, location).map(|u| u.to_string());
        assert_eq!(to("/other").unwrap(), "https://a.example/other");
        assert_eq!(to("next").unwrap(), "https://a.example/docs/next");
        assert_eq!(to("//b.example/x").unwrap(), "https://b.example/x");
        assert_eq!(to("http://c.example/").unwrap(), "http://c.example/");
        assert_eq!(
            to("https://d.example/p#frag").unwrap(),
            "https://d.example/p#frag"
        );
        for bad in [
            "file:///etc/passwd",
            "ftp://a.example/x",
            "javascript:alert(1)",
            "data:text/html,hi",
            "gopher://a.example/",
        ] {
            assert!(matches!(to(bad), Err(FetchError::Scheme(_))), "{bad}");
        }
        assert_eq!(
            to("https://user:pw@e.example/"),
            Err(FetchError::Credentials)
        );
        assert!(matches!(to(""), Err(FetchError::BadRedirect(_))));
        assert!(matches!(to("http://"), Err(FetchError::BadRedirect(_))));
    }

    #[test]
    fn refusals_say_why_in_plain_words() {
        let blocked = FetchError::Blocked {
            host: "intranet.example".into(),
            ip: "10.1.2.3".parse().unwrap(),
            class: AddressClass::Private,
        };
        let text = blocked.to_string();
        assert!(
            text.contains("intranet.example") && text.contains("private network"),
            "{text}"
        );
        assert!(
            text.contains("10.1.2.3") && text.contains("never read"),
            "{text}"
        );
        assert!(blocked.is_refusal());
        assert!(
            FetchError::LocalName("localhost".into())
                .to_string()
                .contains("this computer")
        );
        assert!(
            FetchError::Scheme("file".into())
                .to_string()
                .contains("http and https")
        );
        assert!(FetchError::Timeout(10).to_string().contains("10 seconds"));
        assert!(FetchError::Status(404).to_string().contains("404"));
        assert!(
            FetchError::ContentType("application/pdf".into())
                .to_string()
                .contains("application/pdf")
        );
        assert!(!FetchError::Status(500).is_refusal());
        assert!(!FetchError::Timeout(10).is_refusal());
        assert!(FetchError::Scheme("ftp".into()).is_refusal());
        assert!(FetchError::Credentials.is_refusal());
    }
}
