//! Bounded local Wasm bundles. Loading validates the binary format and
//! retains a snapshot. Host imports belong to the execution contract.

use rustix::fs::{Mode, OFlags};
use std::{fs::File, io::Read, path::Path, sync::Arc};

const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Bundle {
    bytes: Arc<[u8]>,
    component: bool,
    browser: bool,
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
        let browser = component && browser_exports(&bytes)?;
        Ok(Self {
            bytes,
            component,
            browser,
        })
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

    pub fn supports_browser(&self) -> bool {
        self.browser
    }
}

fn browser_exports(bytes: &[u8]) -> Result<bool, String> {
    use wasmparser::{ComponentExternalKind, Parser, Payload};

    let mut depth = 0;
    let mut required = ["browser-open", "browser-click", "browser-back"].map(Some);
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|error| error.to_string())? {
            Payload::Version { .. } => depth += 1,
            Payload::End(_) => depth -= 1,
            Payload::ComponentExportSection(exports) if depth == 1 => {
                for export in exports {
                    let export = export.map_err(|error| error.to_string())?;
                    if export.kind == ComponentExternalKind::Func
                        && let Some(name) = required
                            .iter_mut()
                            .find(|name| **name == Some(export.name.name))
                    {
                        *name = None;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(required.iter().all(Option::is_none))
}
