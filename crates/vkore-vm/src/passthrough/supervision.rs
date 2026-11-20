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
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::net::UnixStream,
        process::{Command, Stdio},
        time::Instant,
    };
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
