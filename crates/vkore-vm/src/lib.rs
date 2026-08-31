//! QEMU lifecycle and exact modern HostLink virtio-serial topology.

use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

pub const CONTROL_PORT_NAME: &str = "org.microperceptron.hostlink.control";
pub const BULK_PORT_NAME: &str = "org.microperceptron.hostlink.bulk";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QemuArgError {
    NonUtf8Path,
    UnsafePath,
}

pub fn arguments(control: &Path, bulk: &Path) -> Result<Vec<String>, QemuArgError> {
    fn safe(path: &Path) -> Result<&str, QemuArgError> {
        let value = path.to_str().ok_or(QemuArgError::NonUtf8Path)?;
        if value.contains([',', '\n', '\r']) {
            return Err(QemuArgError::UnsafePath);
        }
        Ok(value)
    }
    let control = safe(control)?;
    let bulk = safe(bulk)?;
    Ok(vec![
        "-fw_cfg".into(),
        "name=opt/ovmf/X-PciMmio64Mb,string=1024".into(),
        "-chardev".into(),
        format!("socket,id=hostlink_ctl,path={control},reconnect-ms=100"),
        "-chardev".into(),
        format!("socket,id=hostlink_bulk,path={bulk},reconnect-ms=100"),
        "-device".into(),
        "virtio-serial-pci,id=hostlink,bus=pcie.0,disable-legacy=on,iommu_platform=on,max_ports=3,vectors=2".into(),
        "-device".into(),
        format!(
            "virtserialport,bus=hostlink.0,nr=1,name={CONTROL_PORT_NAME},chardev=hostlink_ctl"
        ),
        "-device".into(),
        format!(
            "virtserialport,bus=hostlink.0,nr=2,name={BULK_PORT_NAME},chardev=hostlink_bulk"
        ),
    ])
}

/// Portable loopback TCP topology for Windows hosts. This changes only the
/// QEMU chardev carrier; the guest sees the same two named ports and wire ABI.
pub fn tcp_arguments(control: SocketAddr, bulk: SocketAddr) -> Result<Vec<String>, QemuArgError> {
    if !control.ip().is_loopback()
        || !bulk.ip().is_loopback()
        || control.port() == 0
        || bulk.port() == 0
        || control == bulk
    {
        return Err(QemuArgError::UnsafePath);
    }
    Ok(vec![
        "-fw_cfg".into(),
        "name=opt/ovmf/X-PciMmio64Mb,string=1024".into(),
        "-chardev".into(),
        format!(
            "socket,id=hostlink_ctl,host={},port={},reconnect-ms=100",
            control.ip(),
            control.port()
        ),
        "-chardev".into(),
        format!(
            "socket,id=hostlink_bulk,host={},port={},reconnect-ms=100",
            bulk.ip(),
            bulk.port()
        ),
        "-device".into(),
        "virtio-serial-pci,id=hostlink,bus=pcie.0,disable-legacy=on,iommu_platform=on,max_ports=3,vectors=2".into(),
        "-device".into(),
        format!(
            "virtserialport,bus=hostlink.0,nr=1,name={CONTROL_PORT_NAME},chardev=hostlink_ctl"
        ),
        "-device".into(),
        format!(
            "virtserialport,bus=hostlink.0,nr=2,name={BULK_PORT_NAME},chardev=hostlink_bulk"
        ),
    ])
}

#[derive(Clone, Debug)]
pub struct QemuConfig {
    pub executable: PathBuf,
    pub kore_iso: PathBuf,
    pub arguments: Vec<OsString>,
    pub hostlink_arguments: Vec<String>,
}

impl QemuConfig {
    pub fn spawn(self) -> io::Result<QemuProcess> {
        if !self.kore_iso.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Kore ISO does not exist: {}", self.kore_iso.display()),
            ));
        }
        let mut command = Command::new(&self.executable);
        command.args(self.arguments);
        command.args(self.hostlink_arguments);
        command.arg("-cdrom").arg(self.kore_iso);
        command.spawn().map(|child| QemuProcess { child })
    }
}

#[derive(Debug)]
pub struct QemuProcess {
    child: Child,
}

impl QemuProcess {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn terminate(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(status);
        }
        self.child.kill()?;
        self.child.wait()
    }
}

impl Drop for QemuProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_topology_is_loopback_only_and_keeps_two_named_ports() {
        let control = "127.0.0.1:24538".parse().unwrap();
        let bulk = "127.0.0.1:24539".parse().unwrap();
        let args = tcp_arguments(control, bulk).unwrap();
        assert!(args.iter().any(|arg| arg.contains("port=24538")));
        assert!(args.iter().any(|arg| arg.contains(CONTROL_PORT_NAME)));
        assert!(args.iter().any(|arg| arg.contains(BULK_PORT_NAME)));
        assert!(tcp_arguments("0.0.0.0:1".parse().unwrap(), bulk).is_err());
        assert!(tcp_arguments(control, control).is_err());
    }

    #[test]
    fn missing_iso_is_refused_before_qemu_is_started() {
        let config = QemuConfig {
            executable: "qemu-system-x86_64".into(),
            kore_iso: "missing-kore-test-image.iso".into(),
            arguments: Vec::new(),
            hostlink_arguments: Vec::new(),
        };
        assert_eq!(config.spawn().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn supervised_process_exit_is_observed() {
        let directory = tempfile::tempdir().unwrap();
        let iso = directory.path().join("kore.iso");
        std::fs::write(&iso, []).unwrap();
        let config = QemuConfig {
            executable: "/bin/sh".into(),
            kore_iso: iso,
            arguments: vec!["-c".into(), "exit 0".into()],
            hostlink_arguments: Vec::new(),
        };
        let mut process = config.spawn().unwrap();
        let status = loop {
            if let Some(status) = process.try_wait().unwrap() {
                break status;
            }
            std::thread::yield_now();
        };
        assert!(status.success());
    }
}
