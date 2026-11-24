//! One versioned entry in the current user's credential store per state root.
//! The canonical root is hashed so the entry label exposes no filesystem path.

use std::{os::unix::ffi::OsStrExt, path::Path, sync::Arc};

use keyring_core::{CredentialStore, Entry, Error};

use super::{MAX_RESPONSE, Session};

const SERVICE: &str = "com.microperceptron.vkore.download-account.v1";
pub(super) const UNAVAILABLE: &str = "Secure credential storage is unavailable. Unlock or start your system keyring to restore sign-in.";
pub(super) const INVALID: &str = "The saved sign-in is invalid. Sign in again to replace it.";

#[cfg(target_os = "macos")]
pub(super) const PROVIDER: &str = "macOS Keychain";
#[cfg(target_os = "linux")]
pub(super) const PROVIDER: &str = "Linux Secret Service";
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) const PROVIDER: &str = "Unavailable on this platform";

pub(super) enum Failure {
    Invalid,
    Unavailable,
}

pub(super) struct Keyring {
    scope: String,
    #[cfg(test)]
    test_credential: Option<Arc<keyring_core::Credential>>,
}

impl Keyring {
    pub(super) fn for_root(root: &Path) -> Self {
        Self {
            scope: blake3::hash(root.as_os_str().as_bytes())
                .to_hex()
                .to_string(),
            #[cfg(test)]
            test_credential: None,
        }
    }

    #[cfg(test)]
    pub(super) fn with_test_credential(
        mut self,
        credential: Arc<keyring_core::Credential>,
    ) -> Self {
        self.test_credential = Some(credential);
        self
    }

    fn entry(&self) -> Result<Entry, Failure> {
        // Reconnect on demand. A desktop vault can become available (or its
        // service can restart) while vKore is running.
        #[cfg(test)]
        if let Some(credential) = &self.test_credential {
            return Ok(Entry::new_with_credential(credential.clone()));
        }
        let store = native_store().map_err(|_| Failure::Unavailable)?;
        Entry::new_in_store(&store, SERVICE, &self.scope).map_err(|_| Failure::Unavailable)
    }

    pub(super) fn load(&self) -> Result<Option<Session>, Failure> {
        Self::read(&self.entry()?)
    }

    fn read(entry: &Entry) -> Result<Option<Session>, Failure> {
        match entry.get_password() {
            Ok(data) if data.len() as u64 <= MAX_RESPONSE => serde_json::from_str(&data)
                .map(Some)
                .map_err(|_| Failure::Invalid),
            Ok(_) | Err(Error::BadEncoding(_)) => Err(Failure::Invalid),
            Err(Error::NoEntry) => Ok(None),
            Err(_) => Err(Failure::Unavailable),
        }
    }

    pub(super) fn save(&self, session: &Session) -> Result<(), Failure> {
        let data = serde_json::to_string(session).map_err(|_| Failure::Invalid)?;
        if data.len() as u64 > MAX_RESPONSE {
            return Err(Failure::Invalid);
        }
        self.entry()?
            .set_password(&data)
            .map_err(|_| Failure::Unavailable)
    }

    pub(super) fn take(&self) -> (Option<Session>, bool) {
        let Ok(entry) = self.entry() else {
            return (None, false);
        };
        let saved = Self::read(&entry).ok().flatten();
        let removed = matches!(entry.delete_credential(), Ok(()) | Err(Error::NoEntry));
        (saved, removed)
    }
}

#[cfg(target_os = "macos")]
fn native_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "linux")]
fn native_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn native_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NoDefaultStore)
}
