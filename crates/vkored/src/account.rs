//! Optional download credentials scoped to a vKore state directory. Constructing
//! an account performs no I/O. Only first-party HTTPS requests can carry the credential; redirects
//! are disabled. The server checks identity and current acceptance again before
//! serving any image bytes, independently of this open-source client.
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

mod keyring;
use keyring::Keyring;

pub const ORIGIN: &str = "https://dev.microperceptron.com";
pub const ACCOUNT_URL: &str = "https://dev.microperceptron.com/download-account";
const API: &str = "https://dev.microperceptron.com/api/v1/download-account";
const MAX_RESPONSE: u64 = 32 * 1024;
const REQUIRED: &str = "Image downloads require sign-in and agreement review. Open Settings > Account, or use --sign-in with the CLI create command.";

#[derive(Clone, Default)]
pub struct Account(Arc<LocalAccount>);

#[derive(Default)]
struct LocalAccount {
    keyring: Option<Keyring>,
    access: Mutex<Access>,
    // Views only read this snapshot. System keychain calls can prompt or
    // wait for IPC and must never hold the lock used by the render loop.
    display: Mutex<Display>,
}

#[derive(Default)]
enum Access {
    #[default]
    Saved,
    Temporary(Option<Session>),
    SignOutIncomplete,
}

#[derive(Default)]
struct Display {
    status: Option<Status>,
    persistence: CredentialPersistence,
}

/// Last observed local credential state. This says nothing about server
/// authorization, which is checked independently before a download.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CredentialPersistence {
    #[default]
    NotChecked,
    NoSavedSignIn,
    Saved,
    Temporary,
    Expired,
    StoreUnavailable,
    Invalid,
    SignOutIncomplete,
}

impl CredentialPersistence {
    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::Temporary => Some(TEMPORARY),
            Self::StoreUnavailable => Some(keyring::UNAVAILABLE),
            Self::Invalid => Some(keyring::INVALID),
            Self::SignOutIncomplete => Some(REMOVE_FAILED),
            _ => None,
        }
    }
}

impl fmt::Debug for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Account([credentials redacted])")
    }
}
#[derive(Clone, Deserialize, Serialize)]
struct Session {
    token: String,
    status: Status,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Receipt {
    pub id: String,
    pub subject: String,
    pub version: String,
    pub sha256: String,
    #[serde(rename = "acceptedAt")]
    pub accepted_at: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
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
        let now = now();
        if self.email.is_empty()
            || self.email.len() > 320
            || self.expires_at <= now
            || self.expires_at > now + 3_660_000
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

const TEMPORARY: &str = "This sign-in is temporary. Unlock or start your system keyring and sign in again to keep it across launches.";
const REMOVE_FAILED: &str =
    "Could not remove the saved sign-in. Unlock your system keyring to finish signing out.";

impl Account {
    /// Bind desktop and CLI account operations to the canonical state root.
    /// Construction is local and does not contact a keyring or the network.
    pub fn for_state(state: &crate::State) -> Self {
        Self::with_keyring(Keyring::for_root(state.root()))
    }

    fn with_keyring(keyring: Keyring) -> Self {
        Self(Arc::new(LocalAccount {
            keyring: Some(keyring),
            ..LocalAccount::default()
        }))
    }

    /// Restore the display state on an account worker, entirely offline.
    pub fn restore(&self) {
        let _ = self.session();
    }

    pub fn status(&self) -> Option<Status> {
        self.0
            .display
            .lock()
            .ok()?
            .status
            .as_ref()
            .filter(|status| status.expires_at > now())
            .cloned()
    }

    pub fn storage_warning(&self) -> Option<&'static str> {
        self.persistence().warning()
    }

    pub fn sign_out_incomplete(&self) -> bool {
        self.persistence() == CredentialPersistence::SignOutIncomplete
    }

    /// Read display metadata without touching the system store.
    pub fn persistence(&self) -> CredentialPersistence {
        let Ok(display) = self.0.display.lock() else {
            return CredentialPersistence::StoreUnavailable;
        };
        if display
            .status
            .as_ref()
            .is_some_and(|status| status.expires_at <= now())
        {
            return CredentialPersistence::Expired;
        }
        display.persistence
    }

    pub fn storage_provider(&self) -> &'static str {
        keyring::PROVIDER
    }

    fn display(&self, status: Option<&Status>, persistence: CredentialPersistence) {
        if let Ok(mut display) = self.0.display.lock() {
            *display = Display {
                status: status.cloned(),
                persistence,
            };
        }
    }

    fn session(&self) -> Result<Option<Session>, &'static str> {
        use CredentialPersistence as Storage;

        let mut access = self
            .0
            .access
            .lock()
            .map_err(|_| "Account state unavailable")?;
        match &mut *access {
            Access::Temporary(temporary) => {
                if let Some(session) = temporary.as_ref().filter(|s| s.status.expires_at > now()) {
                    return Ok(Some(session.clone()));
                }
                // If saving failed, an older account could still be in the
                // vault. Keep this process signed out when its session ends.
                *temporary = None;
                self.display(None, Storage::Expired);
                return Ok(None);
            }
            Access::SignOutIncomplete => return Err(REMOVE_FAILED),
            Access::Saved => {}
        }
        // The keyring is authoritative for saved credentials. A running
        // desktop must see a deletion or replacement from another process
        // before it starts another authenticated request.
        let (session, persistence) = match self.0.keyring.as_ref().map_or(Ok(None), Keyring::load) {
            Ok(None) => (None, Storage::NoSavedSignIn),
            Ok(Some(session)) if session.status.expires_at <= now() => (None, Storage::Expired),
            Ok(Some(session))
                if valid_token(&session.token) && session.status.validate().is_ok() =>
            {
                (Some(session), Storage::Saved)
            }
            Ok(Some(_)) | Err(keyring::Failure::Invalid) => {
                self.display(None, Storage::Invalid);
                return Err(keyring::INVALID);
            }
            Err(keyring::Failure::Unavailable) => {
                self.display(None, Storage::StoreUnavailable);
                return Err(keyring::UNAVAILABLE);
            }
        };
        self.display(session.as_ref().map(|session| &session.status), persistence);
        Ok(session)
    }

    fn remember(&self, session: &Session, cancelled: &AtomicBool) -> Result<bool, String> {
        let mut access = self
            .0
            .access
            .lock()
            .map_err(|_| "Account state unavailable")?;
        if cancelled.load(Ordering::Acquire) {
            return Ok(false);
        }
        // Start committing only after cancellation has been checked. The OS
        // may require its own confirmation to complete the write.
        let saved = self.0.keyring.as_ref().map(|keyring| keyring.save(session));
        let persistence = if matches!(saved, Some(Ok(()))) {
            *access = Access::Saved;
            CredentialPersistence::Saved
        } else {
            *access = Access::Temporary(Some(session.clone()));
            CredentialPersistence::Temporary
        };
        self.display(Some(&session.status), persistence);
        Ok(true)
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
            let session = Session {
                token: grant.access_token,
                status: grant.status.clone(),
            };
            if !self.remember(&session, cancelled)? {
                let _ = revoke(&agent, &session.token);
                return Err("Sign-in cancelled".into());
            }
            return Ok(grant.status);
        }
        Err("Sign-in expired. Start again.".into())
    }
    /// Local credentials are discarded immediately, even if revocation is offline.
    pub fn sign_out(&self) -> Result<(), String> {
        let (temporary, saved, removed) = self.forget()?;
        let mut revocation_failed = false;
        for session in [temporary, saved].into_iter().flatten() {
            if session.status.expires_at > now() && valid_token(&session.token) {
                revocation_failed |= agent(20)
                    .and_then(|agent| revoke(&agent, &session.token))
                    .is_err();
            }
        }
        if !removed {
            return Err(REMOVE_FAILED.into());
        }
        if revocation_failed {
            return Err("Signed out locally. The server could not revoke the previous credential; it expires within one hour.".into());
        }
        Ok(())
    }

    fn forget(&self) -> Result<(Option<Session>, Option<Session>, bool), String> {
        let mut access = self
            .0
            .access
            .lock()
            .map_err(|_| "Account state unavailable")?;
        let temporary = match std::mem::take(&mut *access) {
            Access::Temporary(session) => session,
            _ => None,
        };
        self.display(None, CredentialPersistence::NotChecked);
        let (saved, removed) = self.0.keyring.as_ref().map_or((None, true), Keyring::take);
        if !removed {
            *access = Access::SignOutIncomplete;
        }
        let persistence = if removed {
            CredentialPersistence::NoSavedSignIn
        } else {
            CredentialPersistence::SignOutIncomplete
        };
        self.display(None, persistence);
        Ok((temporary, saved, removed))
    }
    pub(crate) fn catalog_request(&self, address: &str) -> Result<ureq::Request, String> {
        checked_url(address, false)?;
        let mut request = agent(20)?.get(address).set("Accept", "application/json");
        if let Ok(Some(session)) = self.session() {
            request = request.set("Authorization", &format!("Bearer {}", session.token));
        }
        Ok(request)
    }
    /// Called only after cache lookup, and before any image request/staging.
    pub(crate) fn download_request(&self, address: &str) -> Result<ureq::Request, String> {
        checked_url(address, true)?;
        let session = self.session()?.ok_or(REQUIRED)?;
        let authorization = format!("Bearer {}", session.token);
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
fn revoke(agent: &ureq::Agent, token: &str) -> Result<(), String> {
    agent
        .post(&format!("{API}/revoke"))
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .map(|_| ())
        .map_err(|_| "Could not revoke download authorization".into())
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
    use std::path::Path;

    fn test_account() -> (Account, Arc<keyring_core::mock::Cred>) {
        let entry = Arc::new(keyring_core::mock::Cred {
            specifiers: ("test-vkore".into(), "test-account".into()),
            inner: Default::default(),
        });
        (account_using(entry.clone()), entry)
    }

    fn account_using(entry: Arc<keyring_core::mock::Cred>) -> Account {
        Account::with_keyring(Keyring::for_root(Path::new("/test")).with_test_credential(entry))
    }

    fn test_session() -> Session {
        Session {
            token: format!("vkdl_{}", "a".repeat(43)),
            status: Status {
                email: "demo@example.test".into(),
                expires_at: now() + 60_000,
                receipt: Receipt {
                    id: "receipt".into(),
                    subject: "subject".into(),
                    version: "1".into(),
                    sha256: "b".repeat(64),
                    accepted_at: "2026-11-24T00:00:00Z".into(),
                },
            },
        }
    }

    #[test]
    fn new_account_process_can_restore_and_forget_an_unexpired_sign_in() {
        let (account, entry) = test_account();
        let session = test_session();
        assert_eq!(account.persistence(), CredentialPersistence::NotChecked);
        assert!(account.remember(&session, &AtomicBool::new(false)).unwrap());
        assert!(matches!(*account.0.access.lock().unwrap(), Access::Saved));
        assert_eq!(account.persistence(), CredentialPersistence::Saved);
        drop(account);

        let restored = account_using(entry.clone());
        restored.restore();
        assert_eq!(restored.status().unwrap().email, session.status.email);
        assert_eq!(restored.persistence(), CredentialPersistence::Saved);
        assert_eq!(restored.session().unwrap().unwrap().token, session.token);

        let other = account_using(entry.clone());
        let (_, saved, removed) = other.forget().unwrap();
        assert!(removed);
        assert_eq!(saved.unwrap().token, session.token);
        assert!(restored.session().unwrap().is_none());
        assert!(restored.status().is_none());
        assert_eq!(restored.persistence(), CredentialPersistence::NoSavedSignIn);
        assert!(account_using(entry).session().unwrap().is_none());
    }

    #[test]
    fn expired_or_corrupt_keyring_data_cannot_authorize_a_request() {
        let (account, entry) = test_account();
        let mut session = test_session();
        session.status.expires_at = 1;
        let entry = keyring_core::Entry::new_with_credential(entry);
        entry
            .set_password(&serde_json::to_string(&session).unwrap())
            .unwrap();
        assert!(account.session().unwrap().is_none());
        assert_eq!(account.persistence(), CredentialPersistence::Expired);
        entry.set_password("not a session").unwrap();
        assert_eq!(account.session().err(), Some(keyring::INVALID));
        assert_eq!(account.persistence(), CredentialPersistence::Invalid);
        assert!(account.status().is_none());
    }

    #[test]
    fn unavailable_keyring_falls_back_only_for_the_current_process() {
        let (account, entry) = test_account();
        entry.set_error(keyring_core::Error::NoDefaultStore);
        assert_eq!(account.persistence(), CredentialPersistence::NotChecked);
        account.restore();
        assert_eq!(
            account.persistence(),
            CredentialPersistence::StoreUnavailable
        );
        entry.set_error(keyring_core::Error::NoDefaultStore);
        let session = test_session();
        assert!(account.remember(&session, &AtomicBool::new(false)).unwrap());
        assert_eq!(account.storage_warning(), Some(TEMPORARY));
        assert_eq!(account.persistence(), CredentialPersistence::Temporary);
        assert!(account.session().unwrap().is_some());
        assert!(account_using(entry).session().unwrap().is_none());
    }

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
