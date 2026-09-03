use crate::{
    AddonEntrypoint, AddonPackage, ExecutionDomain, INVOCATION_PROTOCOL_VERSION,
    InvocationApproval, InvocationResult, ResultHandle, ResultSensitivity, ValidatedInvocation,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct RuntimeLimits {
    pub maximum_timeout: Duration,
    pub maximum_output_bytes: usize,
    pub allowed_network_destinations: std::collections::BTreeSet<String>,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            maximum_timeout: Duration::from_secs(30),
            maximum_output_bytes: 1024 * 1024,
            allowed_network_destinations: std::collections::BTreeSet::new(),
        }
    }
}

#[derive(Debug)]
pub enum RuntimeError {
    WrongDomain,
    MissingTool,
    InvalidEntrypoint,
    EscapedPackageRoot,
    PackageHashMismatch,
    InputTooLarge,
    OutputTooLarge,
    Timeout,
    Io(std::io::Error),
    InvalidResult(serde_json::Error),
    AddonFailed(Option<i32>),
    ResultIdentityMismatch,
    NetworkDestinationDenied(String),
    DisclosureDenied(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RuntimeError {}

impl From<std::io::Error> for RuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub struct HostRuntime {
    scratch_root: PathBuf,
    limits: RuntimeLimits,
}

impl HostRuntime {
    pub fn new(
        scratch_root: impl Into<PathBuf>,
        limits: RuntimeLimits,
    ) -> Result<Self, RuntimeError> {
        let scratch_root = scratch_root.into();
        std::fs::create_dir_all(&scratch_root)?;
        Ok(Self {
            scratch_root,
            limits,
        })
    }

    pub fn invoke(
        &self,
        package: &AddonPackage,
        validated: ValidatedInvocation,
    ) -> Result<InvocationResult, RuntimeError> {
        let approval = validated.approval();
        let tool = package
            .manifest
            .tools
            .iter()
            .find(|tool| tool.name == approval.invocation.tool)
            .ok_or(RuntimeError::MissingTool)?;
        if !matches!(tool.domain, ExecutionDomain::Host | ExecutionDomain::Split) {
            return Err(RuntimeError::WrongDomain);
        }
        if let Some(destination) = tool.network_destinations.iter().find(|destination| {
            !self
                .limits
                .allowed_network_destinations
                .contains(*destination)
        }) {
            return Err(RuntimeError::NetworkDestinationDenied(destination.clone()));
        }
        let (executable, arguments) = match &package.manifest.entrypoint {
            AddonEntrypoint::Process {
                executable,
                arguments,
            }
            | AddonEntrypoint::SplitProcess {
                executable,
                arguments,
                ..
            } => (executable, arguments),
            AddonEntrypoint::GuestExecutable { .. } => return Err(RuntimeError::InvalidEntrypoint),
        };
        let executable = resolve_entrypoint(&package.root, executable)?;
        verify_hash(&executable, &package.manifest.package_hash)?;
        let input = serde_json::to_vec(approval).map_err(RuntimeError::InvalidResult)?;
        if input.len() > tool.maximum_input_bytes {
            return Err(RuntimeError::InputTooLarge);
        }
        let invocation_dir = self
            .scratch_root
            .join(safe_component(&approval.invocation.invocation_id));
        std::fs::create_dir(&invocation_dir)?;
        let mut command = Command::new(executable);
        command
            .args(arguments)
            .current_dir(&invocation_dir)
            .env_clear()
            .env(
                "VKORE_ADDON_PROTOCOL",
                INVOCATION_PROTOCOL_VERSION.to_string(),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| RuntimeError::Io(std::io::Error::other("missing stdin")))?
            .write_all(&input)?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| RuntimeError::Io(std::io::Error::other("missing stdout")))?;
        let output_limit = tool
            .maximum_output_bytes
            .min(self.limits.maximum_output_bytes)
            .min(validated.maximum_output_bytes());
        let reader = thread::spawn(move || read_bounded(&mut stdout, output_limit));
        let deadline = Instant::now()
            + Duration::from_millis(tool.timeout_ms).min(self.limits.maximum_timeout);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = std::fs::remove_dir_all(&invocation_dir);
                return Err(RuntimeError::Timeout);
            }
            thread::sleep(Duration::from_millis(5));
        };
        let output = reader
            .join()
            .map_err(|_| RuntimeError::Io(std::io::Error::other("reader panicked")))??;
        let _ = std::fs::remove_dir_all(&invocation_dir);
        if !status.success() {
            return Err(RuntimeError::AddonFailed(status.code()));
        }
        let result: InvocationResult =
            serde_json::from_slice(&output).map_err(RuntimeError::InvalidResult)?;
        if result.protocol_version != INVOCATION_PROTOCOL_VERSION
            || result.handle.invocation_id != approval.invocation.invocation_id
            || result.handle.agent != approval.invocation.agent
            || result.handle.package_hash != approval.invocation.package_hash
        {
            return Err(RuntimeError::ResultIdentityMismatch);
        }
        if result.handle.byte_count > output_limit || output.len() > output_limit {
            return Err(RuntimeError::OutputTooLarge);
        }
        if let Some(fields) = result.disclosed.as_object()
            && let Some(field) = fields
                .keys()
                .find(|field| !validated.disclosure_fields().contains(*field))
        {
            return Err(RuntimeError::DisclosureDenied(field.clone()));
        }
        Ok(result)
    }
}

fn resolve_entrypoint(root: &Path, entrypoint: &str) -> Result<PathBuf, RuntimeError> {
    let root = root.canonicalize()?;
    let candidate = root.join(entrypoint).canonicalize()?;
    if !candidate.starts_with(&root) {
        return Err(RuntimeError::EscapedPackageRoot);
    }
    Ok(candidate)
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), RuntimeError> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hasher.finalize().to_hex().as_str() != expected {
        return Err(RuntimeError::PackageHashMismatch);
    }
    Ok(())
}

fn read_bounded(reader: &mut impl Read, limit: usize) -> Result<Vec<u8>, RuntimeError> {
    let mut output = Vec::new();
    reader.take((limit + 1) as u64).read_to_end(&mut output)?;
    if output.len() > limit {
        return Err(RuntimeError::OutputTooLarge);
    }
    Ok(output)
}

fn safe_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

impl InvocationResult {
    pub fn from_addon(
        approval: &InvocationApproval,
        sensitivity: ResultSensitivity,
        disclosed: Value,
        available_fields: impl IntoIterator<Item = String>,
    ) -> Self {
        let encoded_len = serde_json::to_vec(&disclosed).map_or(0, |value| value.len());
        let handle = ResultHandle {
            id: format!("result:{}", approval.invocation.invocation_id),
            invocation_id: approval.invocation.invocation_id.clone(),
            agent: approval.invocation.agent.clone(),
            package_hash: approval.invocation.package_hash.clone(),
            sensitivity,
            available_fields: available_fields.into_iter().collect(),
            byte_count: encoded_len,
        };
        Self {
            protocol_version: INVOCATION_PROTOCOL_VERSION,
            handle,
            disclosed,
            untrusted_annotations: BTreeMap::new(),
        }
    }
}
