//! Where the model is: an endpoint, a model name and, usually, a key.
//!
//! All three come from the environment. The key is held in memory, sent only in
//! the `Authorization` header, never printed and never handed to a front end.

use std::fmt;

use url::{Host as UrlHost, Url};

/// The endpoint's base address, ending where `/chat/completions` is appended
/// (for most providers, the part ending in `/v1`).
pub const BASE_URL: &str = "VKORE_AGENT_BASE_URL";
/// The model's name as the endpoint knows it.
pub const MODEL: &str = "VKORE_AGENT_MODEL";
/// The bearer key; optional, since a local server usually wants none.
pub const API_KEY: &str = "VKORE_AGENT_API_KEY";

const MAX_FIELD: usize = 2048;

/// A secret that does not print itself.
#[derive(Clone)]
pub struct Secret(String);

impl Secret {
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// A checked endpoint.
#[derive(Clone, Debug)]
pub struct Endpoint {
    completions: String,
    host: String,
    local: bool,
    model: String,
    key: Option<Secret>,
}

/// Why there is no endpoint to talk to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotConfigured {
    /// A variable that is required is not set.
    Missing(&'static str),
    /// A variable is set to something that cannot be used.
    Invalid(&'static str, &'static str),
}

impl fmt::Display for NotConfigured {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(f, "{name} is not set"),
            Self::Invalid(name, why) => write!(f, "{name} {why}"),
        }
    }
}

impl std::error::Error for NotConfigured {}

impl Endpoint {
    /// Check an endpoint. Plain `http` is accepted only for this computer: anywhere
    /// else it would send the key, and the machine's contents, in the clear.
    pub fn new(base: &str, model: &str, key: Option<&str>) -> Result<Self, NotConfigured> {
        let invalid = NotConfigured::Invalid;
        if base.len() > MAX_FIELD {
            return Err(invalid(BASE_URL, "is too long"));
        }
        let url = Url::parse(base.trim()).map_err(|_| invalid(BASE_URL, "is not an address"))?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(invalid(BASE_URL, "must not carry a user name or password"));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(invalid(BASE_URL, "must not carry a query or fragment"));
        }
        let host = url
            .host()
            .ok_or_else(|| invalid(BASE_URL, "names no host"))?;
        let local = match &host {
            UrlHost::Domain(name) => *name == "localhost",
            UrlHost::Ipv4(address) => address.is_loopback(),
            UrlHost::Ipv6(address) => address.is_loopback(),
        };
        match url.scheme() {
            "https" => {}
            "http" if local => {}
            "http" => {
                return Err(invalid(
                    BASE_URL,
                    "must use https for anything but this computer",
                ));
            }
            _ => return Err(invalid(BASE_URL, "must be an https or http address")),
        }
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        };
        let model = model.trim();
        if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
            return Err(invalid(MODEL, "must name a model, in at most 256 bytes"));
        }
        let key = match key.map(str::trim).filter(|key| !key.is_empty()) {
            Some(key) if key.len() <= MAX_FIELD && key.bytes().all(|b| b.is_ascii_graphic()) => {
                Some(Secret(key.to_owned()))
            }
            Some(_) => return Err(invalid(API_KEY, "is not a usable key")),
            None => None,
        };
        Ok(Self {
            completions: format!("{}/chat/completions", url.as_str().trim_end_matches('/')),
            host,
            local,
            model: model.to_owned(),
            key,
        })
    }

    /// Read the endpoint from variables, looked up through `read`.
    pub fn from_vars(read: impl Fn(&str) -> Option<String>) -> Result<Self, NotConfigured> {
        let required = |name: &'static str| {
            read(name)
                .filter(|value| !value.trim().is_empty())
                .ok_or(NotConfigured::Missing(name))
        };
        let base = required(BASE_URL)?;
        let model = required(MODEL)?;
        Self::new(&base, &model, read(API_KEY).as_deref())
    }

    pub fn from_env() -> Result<Self, NotConfigured> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    pub(crate) fn completions(&self) -> &str {
        &self.completions
    }

    pub(crate) fn key(&self) -> Option<&Secret> {
        self.key.as_ref()
    }

    /// The host (and port) requests go to.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether the endpoint is on this computer, so nothing leaves it.
    pub fn is_local(&self) -> bool {
        self.local
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hosted_endpoint_needs_https() {
        let endpoint =
            Endpoint::new("https://api.example.com/v1/", "some-model", Some("sk-1")).unwrap();
        assert_eq!(
            endpoint.completions(),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(endpoint.host(), "api.example.com");
        assert!(!endpoint.is_local());
        assert_eq!(
            Endpoint::new("http://api.example.com/v1", "m", None).unwrap_err(),
            NotConfigured::Invalid(BASE_URL, "must use https for anything but this computer")
        );
    }

    #[test]
    fn this_computer_may_use_http_without_a_key() {
        for base in [
            "http://127.0.0.1:8080/v1",
            "http://localhost:11434/v1",
            "http://[::1]:8000/v1",
        ] {
            let endpoint = Endpoint::new(base, "m", None).unwrap();
            assert!(endpoint.is_local(), "{base}");
            assert!(endpoint.key().is_none());
        }
        // A name that merely looks local is not.
        assert!(Endpoint::new("http://localhost.example.com/v1", "m", None).is_err());
    }

    #[test]
    fn addresses_that_would_leak_or_confuse_are_refused() {
        for base in [
            "https://user:secret@api.example.com/v1",
            "https://api.example.com/v1?key=x",
            "file:///etc/passwd",
            "api.example.com/v1",
        ] {
            assert!(Endpoint::new(base, "m", None).is_err(), "{base}");
        }
        assert!(Endpoint::new("https://a.example/v1", "", None).is_err());
        assert!(Endpoint::new("https://a.example/v1", "m", Some("two words")).is_err());
    }

    #[test]
    fn the_key_never_prints() {
        let endpoint = Endpoint::new("https://a.example/v1", "m", Some("sk-very-secret")).unwrap();
        assert!(!format!("{endpoint:?}").contains("very-secret"));
    }

    #[test]
    fn the_environment_is_read_and_missing_values_are_named() {
        let none = Endpoint::from_vars(|_| None).unwrap_err();
        assert_eq!(none, NotConfigured::Missing(BASE_URL));
        let no_model =
            Endpoint::from_vars(|name| (name == BASE_URL).then(|| "https://a.example/v1".into()));
        assert_eq!(no_model.unwrap_err(), NotConfigured::Missing(MODEL));
    }
}
