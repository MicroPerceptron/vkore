//! QEMU lifecycle and exact modern HostLink virtio-serial topology.

use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::time::{Duration, Instant};

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
        // Never mount the mutable build output. A later build can truncate the
        // same inode while QEMU still has it open. Copy (not hard-link) into a
        // private directory whose lifetime follows this exact process.
        let image_directory = tempfile::Builder::new().prefix("vkore-image.").tempdir()?;
        let image = image_directory.path().join("kore.iso");
        std::fs::copy(&self.kore_iso, &image)?;
        let mut command = Command::new(&self.executable);
        command.args(self.arguments);
        command.args(self.hostlink_arguments);
        command.arg("-cdrom").arg(image);
        #[cfg(unix)]
        let qmp = {
            let directory = tempfile::Builder::new().prefix("vkore-qmp.").tempdir()?;
            let socket = directory.path().join("control.sock");
            let socket_text = socket.to_str().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "QMP path is not UTF-8")
            })?;
            if socket_text.contains([',', '\n', '\r']) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "QMP path contains a QEMU separator",
                ));
            }
            command
                .arg("-qmp")
                .arg(format!("unix:{socket_text},server=on,wait=off"));
            QmpControl {
                _directory: directory,
                socket,
            }
        };
        command.spawn().map(|child| QemuProcess {
            child,
            _image_directory: image_directory,
            #[cfg(unix)]
            qmp,
        })
    }
}

#[cfg(unix)]
#[derive(Debug)]
struct QmpControl {
    // Keeping the directory alive keeps its private socket pathname reserved
    // for exactly this QEMU process.
    _directory: tempfile::TempDir,
    socket: PathBuf,
}

#[derive(Debug)]
pub struct QemuProcess {
    child: Child,
    // Drop runs process retirement before this backing is removed.
    _image_directory: tempfile::TempDir,
    #[cfg(unix)]
    qmp: QmpControl,
}

impl QemuProcess {
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Stop virtual CPUs and virtual time without disturbing QEMU's process,
    /// sockets, or pending HostLink bytes. Host inference can then outlive a
    /// guest-side bounded poll without spending the Kore task's time budget.
    #[cfg(unix)]
    pub fn suspend(&self) -> io::Result<()> {
        self.qmp.execute("stop")
    }

    #[cfg(not(unix))]
    pub fn suspend(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "QEMU suspension is supported only on Unix hosts",
        ))
    }

    /// Resume virtual CPUs previously stopped by [`Self::suspend`].
    #[cfg(unix)]
    pub fn resume(&self) -> io::Result<()> {
        self.qmp.execute("cont")
    }

    #[cfg(not(unix))]
    pub fn resume(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "QEMU resumption is supported only on Unix hosts",
        ))
    }

    pub fn terminate(&mut self) -> io::Result<ExitStatus> {
        if let Some(status) = self.child.try_wait()? {
            return Ok(status);
        }
        self.child.kill()?;
        self.child.wait()
    }
}

#[cfg(unix)]
impl QmpControl {
    fn execute(&self, command: &str) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let stream = loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => break stream,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        };
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut reader = BufReader::new(stream);
        Self::read_until(&mut reader, "\"QMP\"")?;
        Self::write_command(reader.get_mut(), "qmp_capabilities")?;
        Self::read_return(&mut reader)?;
        Self::write_command(reader.get_mut(), command)?;
        Self::read_return(&mut reader)
    }

    fn write_command(stream: &mut UnixStream, command: &str) -> io::Result<()> {
        if !command
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid QMP command",
            ));
        }
        writeln!(stream, "{{\"execute\":\"{command}\"}}")?;
        stream.flush()
    }

    fn read_return(reader: &mut BufReader<UnixStream>) -> io::Result<()> {
        Self::read_until(reader, "\"return\"")
    }

    fn read_until(reader: &mut BufReader<UnixStream>, marker: &str) -> io::Result<()> {
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "QMP closed before acknowledging the command",
                ));
            }
            if line.contains("\"error\"") {
                return Err(io::Error::other(format!("QMP command failed: {line}")));
            }
            if line.contains(marker) {
                return Ok(());
            }
        }
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

    #[cfg(unix)]
    #[test]
    fn running_process_image_survives_source_rebuild_and_removal() {
        let directory = tempfile::tempdir().unwrap();
        let iso = directory.path().join("kore.iso");
        std::fs::write(&iso, b"original boot image").unwrap();
        let process = QemuConfig {
            executable: "/bin/sh".into(),
            kore_iso: iso.clone(),
            arguments: vec!["-c".into(), "exec sleep 30".into()],
            hostlink_arguments: Vec::new(),
        }
        .spawn()
        .unwrap();
        let mounted = process._image_directory.path().join("kore.iso");
        std::fs::write(&iso, b"replacement build").unwrap();
        std::fs::remove_file(&iso).unwrap();
        assert_eq!(std::fs::read(&mounted).unwrap(), b"original boot image");
        drop(process);
        assert!(!mounted.exists());
    }

    #[cfg(unix)]
    #[test]
    fn qmp_control_negotiates_capabilities_before_stopping_the_vm() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("qmp.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            writeln!(stream, "{{\"QMP\":{{\"version\":{{}}}}}}").unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("qmp_capabilities"));
            writeln!(stream, "{{\"return\":{{}}}}").unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("\"stop\""));
            writeln!(stream, "{{\"return\":{{}}}}").unwrap();
        });
        let control = QmpControl {
            _directory: directory,
            socket,
        };
        control.execute("stop").unwrap();
        server.join().unwrap();
    }
}
