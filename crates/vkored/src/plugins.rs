//! Bounded local Wasm bundles. Loading validates the binary format and
//! retains a snapshot. Host imports belong to the execution contract.

use rustix::fs::{Mode, OFlags};
use std::{fs::File, io::Read, path::Path, sync::Arc};

const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Bundle {
    bytes: Arc<[u8]>,
    component: bool,
}

impl Bundle {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = rustix::fs::open(path, OFlags::RDONLY | OFlags::NONBLOCK, Mode::empty())
            .map(File::from)
            .map_err(|error| error.to_string())?;
        if !file
            .metadata()
            .map_err(|error| error.to_string())?
            .is_file()
        {
            return Err("Choose a regular Wasm file.".into());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BUNDLE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        Self::validate(bytes.into())
    }

    pub fn validate(bytes: Arc<[u8]>) -> Result<Self, String> {
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err("The Wasm bundle exceeds 32 MiB.".into());
        }
        let component = wasmparser::Parser::is_component(&bytes);
        if !component && !wasmparser::Parser::is_core_wasm(&bytes) {
            return Err("Choose a WebAssembly component or module.".into());
        }
        wasmparser::Validator::new()
            .validate_all(&bytes)
            .map_err(|error| error.to_string())?;
        Ok(Self { bytes, component })
    }

    pub fn kind(&self) -> &'static str {
        if self.component {
            "Wasm component"
        } else {
            "Wasm module"
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
