//! The device guardian must reap the guest on every return path. Device-driver
//! guards are owned by its caller and are therefore dropped only afterwards.
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    os::fd::AsFd,
    process::{Child, ExitStatus},
    time::Duration,
};
struct Guest(Child);
impl Drop for Guest {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
pub(super) fn supervise(
    child: Child,
    control: impl AsFd,
    stopped: impl Fn() -> bool,
) -> Result<ExitStatus, String> {
    let mut guest = Guest(child);
    loop {
        if let Some(status) = guest.0.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        if stopped() {
            let _ = guest.0.kill();
            return guest.0.wait().map_err(|e| e.to_string());
        }
        let mut fds = [PollFd::new(&control, PollFlags::IN)];
        match poll(
            &mut fds,
            Some(&Timespec {
                tv_sec: 0,
                tv_nsec: 50_000_000,
            }),
        ) {
            Ok(_) if !fds[0].revents().is_empty() => {
                let _ = guest.0.kill();
                return guest.0.wait().map_err(|e| e.to_string());
            }
            Ok(_) => (),
            Err(e) if e == rustix::io::Errno::INTR => std::thread::sleep(Duration::from_millis(1)),
            Err(e) => return Err(e.to_string()), // Guest Drop kills and reaps first.
        }
    }
}
/// A successful probe realizes all devices but never executes guest code.
/// Always reap the emulator before its caller can reopen VFIO descriptors.
pub(super) fn probe_paused(
    child: Child,
    stream: std::os::unix::net::UnixStream,
) -> Result<(), String> {
    let mut guest = Guest(child);
    let mut qmp =
        crate::qmp::Qmp::from_stream(stream, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let status = qmp.status().map_err(|e| e.to_string())?;
    if status.running || status.status != qapi_qmp::RunState::prelaunch {
        return Err("IOMMU probe did not stay in prelaunch state.".into());
    }
    qmp.quit().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = guest.0.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("IOMMU probe exited with {status}."))
            };
        }
        if std::time::Instant::now() >= deadline {
            return Err("IOMMU probe did not exit after QMP quit.".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::net::UnixStream,
        process::{Command, Stdio},
        time::Instant,
    };
    #[test]
    fn probe_rejects_running_cpus_and_reaps_before_returning() {
        use std::io::{BufRead, BufReader, Write};
        let (client, mut server) = UnixStream::pair().unwrap();
        let peer = std::thread::spawn(move || {
            writeln!(server, "{}", serde_json::json!({"QMP":{"version":{"qemu":{"major":10,"minor":2,"micro":1},"package":"fixture"},"capabilities":[]}})).unwrap();
            let mut reader = BufReader::new(server.try_clone().unwrap());
            for expected in ["qmp_capabilities", "query-status"] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["execute"], expected);
                let result = if expected == "query-status" {
                    serde_json::json!({"running":true,"singlestep":false,"status":"running"})
                } else {
                    serde_json::json!({})
                };
                writeln!(
                    server,
                    "{}",
                    serde_json::json!({"return":result,"id":request["id"]})
                )
                .unwrap();
            }
        });
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = rustix::process::Pid::from_raw(child.id() as i32).unwrap();
        assert!(
            probe_paused(child, client)
                .unwrap_err()
                .contains("prelaunch")
        );
        assert_eq!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH)
        );
        peer.join().unwrap();
    }
    #[test]
    fn parent_disconnect_stops_and_reaps_the_guest() {
        let (parent, guardian) = UnixStream::pair().unwrap();
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        drop(parent);
        assert!(!supervise(child, guardian, || false).unwrap().success());
        assert!(start.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn termination_request_stops_guest_without_waiting_for_pipe_eof() {
        let (_parent, guardian) = UnixStream::pair().unwrap();
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        assert!(!supervise(child, guardian, || true).unwrap().success());
    }
    #[test]
    fn natural_exit_does_not_wait_for_the_parent_to_disconnect() {
        let (_parent, guardian) = UnixStream::pair().unwrap();
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        assert_eq!(
            supervise(child, guardian, || false).unwrap().code(),
            Some(7)
        );
    }
}
