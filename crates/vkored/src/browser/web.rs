use std::{
    io::{self, Read},
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};

use url::{Host, Url};
use wasmtime::component::{ComponentType, Lift, Lower};

pub const MAX_ADDRESS: usize = 2048;
const MIB: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

#[derive(Clone, Copy, ComponentType, Lift)]
#[expect(
    dead_code,
    reason = "Constructed by the component ABI when lifting the WIT enum"
)]
#[component(enum)]
#[repr(u8)]
pub(super) enum Resource {
    #[component(name = "document")]
    Document,
    #[component(name = "stylesheet")]
    Stylesheet,
    #[component(name = "image")]
    Image,
}

impl Resource {
    fn accept(self) -> &'static str {
        match self {
            Self::Document => "text/html, application/xhtml+xml",
            Self::Stylesheet => "text/css",
            Self::Image => "image/png, image/jpeg, image/gif, image/webp",
        }
    }
}

#[derive(ComponentType, Lower)]
#[component(record)]
pub(super) struct Response {
    location: String,
    bytes: Vec<u8>,
}

pub(super) struct Budget {
    deadline: Instant,
    bytes: usize,
    requests: usize,
}

impl Budget {
    pub(super) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(12),
            bytes: 8 * MIB,
            requests: 24,
        }
    }

    fn request(&mut self) -> Result<Duration, String> {
        if self.requests == 0 || self.bytes == 0 {
            return Err("The page reached the browser's download limit.".into());
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("The page took too long to load.".into());
        }
        self.requests -= 1;
        Ok(remaining.min(Duration::from_secs(5)))
    }
}

pub(super) fn agent() -> Result<ureq::Agent, native_tls::Error> {
    let tls = native_tls::TlsConnector::new()?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .https_only(true)
        .try_proxy_from_env(false)
        .redirects(0)
        .resolver(resolve_public)
        .user_agent("vKore Kitesurf/0.3")
        .build())
}

pub(super) fn fetch(
    agent: &ureq::Agent,
    budget: &mut Budget,
    address: &str,
    resource: Resource,
) -> Result<Response, String> {
    if address.len() > MAX_ADDRESS {
        return Err("The address is too long.".into());
    }
    let mut url = Url::parse(address).map_err(|_| "Enter a valid HTTPS address.")?;
    for hop in 0..=MAX_REDIRECTS {
        validate_url(&url)?;
        let timeout = budget.request()?;
        let response = agent
            .get(url.as_str())
            .timeout(timeout)
            .set("Accept", resource.accept())
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => format!("The website returned HTTP {status}."),
                ureq::Error::Transport(error) => format!(
                    "Could not connect to {} ({:?}).",
                    url.host_str().unwrap_or_default(),
                    error.kind()
                ),
            })?;
        match response.status() {
            301 | 302 | 303 | 307 | 308 if hop < MAX_REDIRECTS => {
                let location = response
                    .header("Location")
                    .ok_or("The website sent a redirect without an address.")?;
                url = url
                    .join(location)
                    .map_err(|_| "Invalid redirect address.")?;
            }
            301 | 302 | 303 | 307 | 308 => {
                return Err("The website sent too many redirects.".into());
            }
            200 => return read_body(budget, url, resource, response),
            status => return Err(format!("The website returned HTTP {status}.")),
        }
    }
    unreachable!("the final hop returns a response or an error")
}

fn read_body(
    budget: &mut Budget,
    url: Url,
    resource: Resource,
    response: ureq::Response,
) -> Result<Response, String> {
    let mime = response.content_type().to_ascii_lowercase();
    if !resource.accept().split(", ").any(|kind| kind == mime) {
        return Err("This content type is unsupported by the browser.".into());
    }
    if response
        .header("Content-Encoding")
        .is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
    {
        return Err("The website returned an unsupported content encoding.".into());
    }
    let cap = budget.bytes.min(2 * MIB);
    let mut bytes = Vec::new();
    let result = response
        .into_reader()
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes);
    budget.bytes = budget.bytes.saturating_sub(bytes.len());
    result.map_err(|_| "The connection ended before the page was loaded.")?;
    if bytes.len() > cap {
        return Err("The page reached the browser's download limit.".into());
    }
    Ok(Response {
        location: url.into(),
        bytes,
    })
}

fn validate_url(url: &Url) -> Result<(), String> {
    if url.as_str().len() > MAX_ADDRESS {
        return Err("The address is too long.".into());
    }
    let public_host = matches!(url.host(), Some(Host::Domain(host)) if {
        let host = host.trim_end_matches('.');
        host.contains('.') && !host.ends_with(".localhost") && !host.ends_with(".local")
    });
    if url.scheme() != "https"
        || !public_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err("Enter a public HTTPS address without credentials or a custom port.".into());
    }
    Ok(())
}

// Filter the exact addresses the HTTP client can connect to, including after redirects.
fn resolve_public(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    let addresses: Vec<_> = netloc
        .to_socket_addrs()?
        .filter(|address| is_public(address.ip()))
        .collect();
    if addresses.is_empty() {
        return Err(io::Error::other("No public address for this hostname"));
    }
    Ok(addresses)
}

fn is_public(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => !matches!(
            ip.octets(),
            [0 | 10 | 127 | 224..=255, ..]
                | [100, 64..=127, ..]
                | [169, 254, ..]
                | [172, 16..=31, ..]
                | [192, 0, 0 | 2, _]
                | [192, 88, 99, _]
                | [192, 168, ..]
                | [198, 18 | 19, ..]
                | [198, 51, 100, _]
                | [203, 0, 113, _]
        ),
        IpAddr::V6(ip) => {
            matches!(ip.segments(), [0x2000..=0x3fff, ..])
                && !matches!(
                    ip.segments(),
                    [0x2001, 0..=0x1ff | 0xdb8, ..] | [0x2002 | 0x3fff, ..]
                )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_public_https_urls_are_allowed() {
        assert!(validate_url(&Url::parse("https://example.com/path#anchor").unwrap()).is_ok());
        for address in [
            "http://example.com/",
            "https://127.0.0.1/",
            "https://[::ffff:127.0.0.1]/",
            "https://test.localhost/",
            "https://localhost./",
            "https://example.com:8080/",
            "https://user:secret@example.com/",
        ] {
            assert!(
                validate_url(&Url::parse(address).unwrap()).is_err(),
                "{address}"
            );
        }
    }

    #[test]
    fn resolver_excludes_non_public_addresses() {
        for address in [
            "127.0.0.1",
            "10.0.1.0",
            "100.64.0.1",
            "172.16.0.1",
            "169.254.169.254",
            "192.168.1.1",
            "198.18.0.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:a00:1::1",
        ] {
            assert!(!is_public(address.parse().unwrap()), "{address}");
        }
        assert!(is_public("1.1.1.1".parse().unwrap()));
        assert!(is_public("2606:4700:4700::1111".parse().unwrap()));
        assert!(resolve_public("127.0.0.1:443").is_err());
    }
}
