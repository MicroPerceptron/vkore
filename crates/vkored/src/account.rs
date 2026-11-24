//! Optional, memory-only download credentials. Constructing an account performs
//! no I/O. Only first-party HTTPS requests can carry the credential; redirects
//! are disabled. The server checks identity and current acceptance again before
//! serving any image bytes, independently of this open-source client.
use serde::Deserialize;
use std::{
    fmt,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const ORIGIN: &str = "https://dev.microperceptron.com";
pub const ACCOUNT_URL: &str = "https://dev.microperceptron.com/download-account";
const API: &str = "https://dev.microperceptron.com/api/v1/download-account";
const MAX_RESPONSE: u64 = 32 * 1024;
const REQUIRED: &str = "Image downloads require sign-in and agreement review. Open Settings > Account, or use --sign-in with the CLI create command.";

#[derive(Clone, Default)]
pub struct Account(Arc<Mutex<Option<Session>>>);
impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Account([credentials redacted])")
    }
}
struct Session {
    token: String,
    status: Status,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Receipt {
    pub id: String,
    pub subject: String,
    pub version: String,
    pub sha256: String,
    #[serde(rename = "acceptedAt")]
    pub accepted_at: String,
}
#[derive(Debug, Clone, Deserialize)]
pub struct Status {
    pub email: String,
    pub expires_at: u64,
    pub receipt: Receipt,
}
#[derive(Deserialize)]
struct Device {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}
#[derive(Deserialize)]
struct Granted {
    access_token: String,
    token_type: String,
    #[serde(flatten)]
    status: Status,
}
#[derive(Debug, Clone)]
pub struct SignIn {
    pub code: String,
    pub url: String,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn agent(timeout: u64) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|_| "Could not initialize secure sign-in")?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .https_only(true)
        .redirects(0)
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .timeout(Duration::from_secs(timeout))
        .build())
}
fn read<T: serde::de::DeserializeOwned>(response: ureq::Response) -> Result<T, String> {
    if response.status() != 200 {
        return Err("The account service returned an unexpected response".into());
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Account response interrupted")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("Account response exceeds its size limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid account response".into())
}
fn checked_url(address: &str, download: bool) -> Result<(), String> {
    let base = url::Url::parse(ORIGIN).unwrap();
    let url = url::Url::parse(address).map_err(|_| "Invalid image service address")?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Downloads must use the official image service; import other images from a local file"
                .into(),
        );
    }
    if download {
        let parts: Vec<_> = url.path().split('/').collect();
        if url.query().is_some()
            || parts.len() != 4
            || parts[1] != "dl"
            || parts[2].is_empty()
            || !parts[3].ends_with(".iso")
            || !parts[2..].iter().all(|p| {
                p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
        {
            return Err("Use an official image download URL without query credentials".into());
        }
    } else if url.path() != "/api/v1/archives" {
        return Err("Invalid archive metadata address".into());
    }
    Ok(())
}
impl Status {
    fn validate(&self) -> Result<(), String> {
        let r = &self.receipt;
        if self.email.is_empty()
            || self.email.len() > 320
            || self.expires_at <= now()
            || self.expires_at > now() + 3_660_000
            || r.id.is_empty()
            || r.id.len() > 128
            || r.subject.is_empty()
            || r.subject.len() > 128
            || r.version.is_empty()
            || r.version.len() > 128
            || r.sha256.len() != 64
            || !r.sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || r.accepted_at.is_empty()
            || r.accepted_at.len() > 64
        {
            return Err("Invalid or expired account authorization".into());
        }
        Ok(())
    }
}
impl Account {
    pub fn status(&self) -> Option<Status> {
        self.0
            .lock()
            .ok()?
            .as_ref()
            .filter(|s| s.status.expires_at > now())
            .map(|s| s.status.clone())
    }
    /// The callback contains only the public user code; never the polling secret.
    pub fn sign_in(&self, cancelled: &AtomicBool, show: impl Fn(SignIn)) -> Result<Status, String> {
        let agent = agent(20)?;
        let device: Device = read(agent.post(&format!("{API}/device")).set("Content-Type", "application/json").send_string("{}").map_err(|_| "Cannot start sign-in. Check your connection and that the account service is deployed.")?)?;
        if device.device_code.len() != 43
            || device.user_code.len() != 12
            || !device.user_code.bytes().all(|b| b.is_ascii_hexdigit())
            || device.verification_uri != "/download-account"
            || !(1..=600).contains(&device.expires_in)
            || !(3..=30).contains(&device.interval)
        {
            return Err("Invalid sign-in challenge".into());
        }
        show(SignIn {
            code: device.user_code,
            url: format!("{ACCOUNT_URL}?device=1"),
        });
        let deadline = Instant::now() + Duration::from_secs(device.expires_in);
        while Instant::now() < deadline {
            for _ in 0..device.interval * 10 {
                if cancelled.load(Ordering::Acquire) {
                    return Err("Sign-in cancelled".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let response = agent
                .post(&format!("{API}/token"))
                .set("Content-Type", "application/json")
                .send_string(&serde_json::json!({"device_code":device.device_code}).to_string());
            let response = match response {
                Ok(r) if r.status() == 202 => continue,
                Ok(r) => r,
                Err(ureq::Error::Status(429, _)) => continue,
                Err(_) => {
                    return Err("Sign-in expired or could not be completed. Try again.".into());
                }
            };
            let grant: Granted = read(response)?;
            grant.status.validate()?;
            if grant.token_type != "Bearer" || !valid_token(&grant.access_token) {
                return Err("Invalid download credential".into());
            }
            let mut session = self.0.lock().map_err(|_| "Account state unavailable")?;
            if cancelled.load(Ordering::Acquire) {
                drop(session);
                let _ = agent
                    .post(&format!("{API}/revoke"))
                    .set("Authorization", &format!("Bearer {}", grant.access_token))
                    .call();
                return Err("Sign-in cancelled".into());
            }
            *session = Some(Session {
                token: grant.access_token,
                status: grant.status.clone(),
            });
            return Ok(grant.status);
        }
        Err("Sign-in expired. Start again.".into())
    }
    /// Local credentials are discarded immediately, even if revocation is offline.
    pub fn sign_out(&self) -> Result<(), String> {
        let session = self
            .0
            .lock()
            .map_err(|_| "Account state unavailable")?
            .take();
        if let Some(session) = session {
            agent(20)?.post(&format!("{API}/revoke")).set("Authorization", &format!("Bearer {}", session.token)).call()
                .map_err(|_| "Signed out locally. The server could not be reached; the previous credential expires within one hour.")?;
        }
        Ok(())
    }
    pub(crate) fn catalog_request(&self, address: &str) -> Result<ureq::Request, String> {
        checked_url(address, false)?;
        let mut request = agent(20)?.get(address).set("Accept", "application/json");
        if let Some(session) = self
            .0
            .lock()
            .map_err(|_| "Account state unavailable")?
            .as_ref()
            .filter(|s| s.status.expires_at > now())
        {
            request = request.set("Authorization", &format!("Bearer {}", session.token));
        }
        Ok(request)
    }
    /// Called only after cache lookup, and before any image request/staging.
    pub(crate) fn download_request(&self, address: &str) -> Result<ureq::Request, String> {
        checked_url(address, true)?;
        let guard = self.0.lock().map_err(|_| "Account state unavailable")?;
        let session = guard
            .as_ref()
            .filter(|s| s.status.expires_at > now())
            .ok_or(REQUIRED)?;
        let authorization = format!("Bearer {}", session.token);
        drop(guard);
        let status: Status = read(
            agent(20)?
                .get(&format!("{API}/session"))
                .set("Authorization", &authorization)
                .call()
                .map_err(|_| REQUIRED)?,
        )?;
        status.validate()?;
        Ok(agent(3600)?
            .get(address)
            .set("Authorization", &authorization))
    }
}
fn valid_token(token: &str) -> bool {
    token.len() == 48
        && token.starts_with("vkdl_")
        && token[5..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_account_cannot_request_any_download() {
        let account = Account::default();
        assert!(account.status().is_none());
        assert!(
            account
                .download_request(&format!("{ORIGIN}/dl/release/image.iso"))
                .err()
                .unwrap()
                .contains("sign-in")
        );
        assert!(account.sign_out().is_ok());
    }
    #[test]
    fn credentials_cannot_leave_the_service_or_enter_query_links() {
        for url in [
            "https://evil.test/dl/r/image.iso",
            "http://dev.microperceptron.com/dl/r/image.iso",
            "https://user@dev.microperceptron.com/dl/r/image.iso",
            "https://dev.microperceptron.com/dl/r/image.iso?token=secret",
            "https://dev.microperceptron.com/api/redirect.iso",
            "https://dev.microperceptron.com/dl/r/%2fimage.iso",
        ] {
            assert!(checked_url(url, true).is_err(), "{url}");
        }
        assert!(checked_url(&format!("{ORIGIN}/dl/r/image.iso"), true).is_ok());
        assert!(!format!("{:?}", Account::default()).contains("vkdl_"));
    }
}
