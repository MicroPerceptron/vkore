//! A read-only, one-disk broker. Polkit authorizes the executable, not a
//! generated shell command. No host filenames, writes or shell commands are
//! accepted after admission. Kore's engine retains the disk lock throughout.

#[cfg(any(target_os = "linux", test))]
use {crate::Browser, serde::de::DeserializeOwned, std::io::Read};

use crate::{Error, Listing, MountStatus, VolPath, Walked};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    process::Child,
    sync::{Mutex, mpsc},
    thread,
    time::Duration,
};

const FLAG: &str = "--vkore-volume-reader";
const CHUNK: usize = 64 << 10;
// A walk holds at most MAX_OBJECTS paths, each at most MAX_PATH_DEPTH names.
const FRAME: usize = 8 << 20;
const REQUEST: usize = 8 << 10;
const WAIT: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize)]
struct Path {
    face: u8,
    text: String,
}
impl From<&VolPath> for Path {
    fn from(path: &VolPath) -> Self {
        Self {
            face: path.face(),
            text: path.to_string(),
        }
    }
}
impl Path {
    #[cfg(any(target_os = "linux", test))]
    fn checked(&self) -> Result<VolPath, Error> {
        VolPath::parse(self.face, &self.text)
    }
}
#[derive(Serialize, Deserialize)]
enum Request {
    List(Path),
    Walk(Path),
    Head(Path, usize),
    Copy(Path),
    Close,
}
#[derive(Serialize, Deserialize)]
enum Response {
    Ready(MountStatus),
    Listing(Listing),
    Walk(Vec<Walked>),
    Bytes(Vec<u8>),
    End,
    Failed(String),
}

fn failure(message: impl Into<String>) -> Error {
    Error::Path(message.into())
}
fn wire(error: impl std::fmt::Display) -> Error {
    failure(format!("Volume authorization session: {error}"))
}

fn send(writer: &mut impl Write, message: &impl Serialize, limit: usize) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    if bytes.len() > limit {
        return Err(io::Error::other("volume message exceeds its bound"));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}
#[cfg(any(target_os = "linux", test))]
fn receive<T: DeserializeOwned>(reader: &mut impl Read, limit: usize) -> io::Result<T> {
    let mut size = [0; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    if size > limit {
        return Err(io::Error::other("volume message exceeds its bound"));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

struct Client {
    input: Option<Box<dyn Write + Send>>,
    replies: mpsc::Receiver<io::Result<Response>>,
    child: Option<Child>,
}
impl Client {
    #[cfg(any(target_os = "linux", test))]
    fn new(
        input: impl Write + Send + 'static,
        mut output: impl Read + Send + 'static,
        child: Option<Child>,
    ) -> io::Result<Self> {
        let (send, replies) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("vkore-volume-replies".into())
            .spawn(move || {
                loop {
                    let response = receive(&mut output, FRAME);
                    let failed = response.is_err();
                    if send.send(response).is_err() || failed {
                        break;
                    }
                }
            })?;
        Ok(Self {
            input: Some(Box::new(input)),
            replies,
            child,
        })
    }
    fn disconnect(&mut self) {
        drop(self.input.take());
        let (_, disconnected) = mpsc::sync_channel(1);
        self.replies = disconnected;
    }

    fn next(&mut self) -> Result<Response, Error> {
        let result = self
            .replies
            .recv_timeout(WAIT)
            .map_err(wire)
            .and_then(|r| r.map_err(wire));
        match result {
            Ok(Response::Failed(message)) => Err(failure(message)),
            Ok(response) => Ok(response),
            Err(error) => {
                self.disconnect();
                Err(error)
            }
        }
    }

    fn request(&mut self, request: &Request) -> Result<Response, Error> {
        send(
            &mut self.input.as_mut().ok_or(Error::SessionEnded)?,
            request,
            REQUEST,
        )
        .map_err(wire)?;
        self.next()
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // EOF releases the helper's mount even if the UI goes away. Reap off
        // the UI/volume worker: a privileged process cannot be killed by its
        // ordinary-user parent, and an auth dialog may still be dismissing.
        drop(self.input.take());
        if let Some(mut child) = self.child.take() {
            let _ = thread::Builder::new()
                .name("vkore-volume-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                });
        }
    }
}

pub(crate) struct Remote(Mutex<Client>);
impl Remote {
    #[cfg(target_os = "linux")]
    pub(crate) fn authorize(disk: &crate::Disk) -> Result<(Self, MountStatus), Error> {
        use std::process::{Command, Stdio};
        let executable = std::env::current_exe().map_err(wire)?;
        let mut child = Command::new("/usr/bin/pkexec")
            .arg("--disable-internal-agent").arg(executable).arg(FLAG)
            .arg(disk.path()).arg(disk.identity().0.to_string()).arg(disk.identity().1.to_string())
            .arg(std::process::id().to_string())
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
            .spawn().map_err(|error| failure(format!(
                "Cannot request disk access: {error}. A desktop polkit authentication agent is required."
            )))?;
        let input = child.stdin.take().ok_or(Error::SessionEnded)?;
        let output = child.stdout.take().ok_or(Error::SessionEnded)?;
        let mut client = Client::new(input, output, Some(child)).map_err(wire)?;
        match client.next() {
            Ok(Response::Ready(status)) => Ok((Self(Mutex::new(client)), status)),
            Ok(_) => Err(wire("unexpected opening reply")),
            Err(error) => Err(failure(format!(
                "Could not open the disk. Authorization may have been cancelled or denied. {error}"
            ))),
        }
    }
    fn client(&self) -> Result<std::sync::MutexGuard<'_, Client>, Error> {
        self.0.lock().map_err(|_| Error::SessionEnded)
    }
    pub(crate) fn list(&self, path: &VolPath) -> Result<Listing, Error> {
        match self.client()?.request(&Request::List(path.into()))? {
            Response::Listing(listing) => Ok(listing),
            _ => Err(wire("unexpected listing reply")),
        }
    }
    pub(crate) fn walk(&self, path: &VolPath) -> Result<Vec<Walked>, Error> {
        match self.client()?.request(&Request::Walk(path.into()))? {
            Response::Walk(walk) => Ok(walk),
            _ => Err(wire("unexpected walk reply")),
        }
    }
    pub(crate) fn head(&self, path: &VolPath, limit: usize) -> Result<Vec<u8>, Error> {
        if limit > CHUNK {
            return Err(failure("Authorized previews are limited to 64 KiB."));
        }
        match self.client()?.request(&Request::Head(path.into(), limit))? {
            Response::Bytes(bytes) if bytes.len() <= limit => Ok(bytes),
            _ => Err(wire("unexpected preview reply")),
        }
    }
    pub(crate) fn read(&self, path: &VolPath, limit: u64) -> Result<Vec<u8>, Error> {
        struct Bounded {
            bytes: Vec<u8>,
            limit: u64,
        }
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if bytes.len() as u64 > self.limit.saturating_sub(self.bytes.len() as u64) {
                    return Err(io::Error::other("read exceeds its bound"));
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        // Refuse large files before starting the transfer.
        if let Listing::File { len } = self.list(path)?
            && len > limit
        {
            return Err(Error::TooLarge {
                what: path.to_string(),
                limit,
            });
        }
        self.copy_to(
            path,
            Bounded {
                bytes: Vec::new(),
                limit,
            },
        )
        .map(|(_, _, sink)| sink.bytes)
    }
    pub(crate) fn copy_to<W: Write>(
        &self,
        path: &VolPath,
        mut sink: W,
    ) -> Result<(u64, blake3::Hash, W), Error> {
        let mut client = self.client()?;
        let mut response = client.request(&Request::Copy(path.into()))?;
        let mut hasher = blake3::Hasher::new();
        let mut len = 0;
        loop {
            match response {
                Response::Bytes(bytes) if bytes.len() <= CHUNK => {
                    if let Err(error) = sink.write_all(&bytes) {
                        // Stop immediately on a full export destination. Closing
                        // both pipes makes the helper release its read-only mount.
                        client.disconnect();
                        return Err(wire(error));
                    }
                    hasher.update(&bytes);
                    len += bytes.len() as u64;
                }
                Response::End => break,
                _ => return Err(wire("unexpected copy reply")),
            }
            response = client.next()?;
        }
        Ok((len, hasher.finalize(), sink))
    }
    pub(crate) fn close(self) -> Result<(), Error> {
        match self.client()?.request(&Request::Close)? {
            Response::End => Ok(()),
            _ => Err(wire("unexpected close reply")),
        }
    }
}

#[cfg(any(target_os = "linux", test))]
fn serve(
    reader: &mut impl Read,
    writer: impl Write + Send + 'static,
    browser: Browser,
) -> io::Result<()> {
    use std::sync::Arc;
    let output = Arc::new(Mutex::new(writer));
    let answer = |response| {
        send(
            &mut *output.lock().map_err(|_| io::Error::other("reply lock"))?,
            &response,
            FRAME,
        )
    };
    answer(Response::Ready(browser.status()))?;
    loop {
        let request = match receive(reader, REQUEST) {
            Ok(request) => request,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        let result = match request {
            Request::List(path) => path
                .checked()
                .and_then(|p| browser.list(&p))
                .map(Response::Listing),
            Request::Walk(path) => path
                .checked()
                .and_then(|p| browser.walk(&p))
                .map(Response::Walk),
            Request::Head(path, limit) if limit <= CHUNK => path
                .checked()
                .and_then(|p| browser.head(&p, limit))
                .map(Response::Bytes),
            Request::Head(_, _) => Err(failure("Preview exceeds the 64 KiB bound.")),
            Request::Copy(path) => {
                struct Sink<W>(Arc<Mutex<W>>);
                impl<W: Write> Write for Sink<W> {
                    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                        let mut output =
                            self.0.lock().map_err(|_| io::Error::other("reply lock"))?;
                        for chunk in bytes.chunks(CHUNK) {
                            send(&mut *output, &Response::Bytes(chunk.to_vec()), FRAME)?;
                        }
                        Ok(bytes.len())
                    }
                    fn flush(&mut self) -> io::Result<()> {
                        Ok(())
                    }
                }
                path.checked()
                    .and_then(|p| browser.copy_to(&p, Sink(output.clone())))
                    .map(|_| Response::End)
            }
            Request::Close => {
                // Acknowledge only after the engine released its disk lock.
                drop(browser);
                return answer(Response::End);
            }
        };
        answer(result.unwrap_or_else(|error| Response::Failed(error.to_string())))?;
    }
}

/// Call before starting a desktop event loop. This is a private, read-only
/// polkit entry point, not a user-facing command or an arbitrary root runner.
pub fn helper_entry() -> Option<i32> {
    if std::env::args_os().nth(1).as_deref() != Some(std::ffi::OsStr::new(FLAG)) {
        return None;
    }
    #[cfg(target_os = "linux")]
    let result = admit()
        .and_then(|disk| Browser::open(&disk))
        .and_then(|browser| serve(&mut io::stdin().lock(), io::stdout(), browser).map_err(wire));
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), Error> = Err(failure("Physical disk authorization requires Linux."));
    Some(match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = send(
                &mut io::stdout(),
                &Response::Failed(error.to_string()),
                FRAME,
            );
            1
        }
    })
}

#[cfg(target_os = "linux")]
fn admit() -> Result<crate::Disk, Error> {
    use rustix::process::{geteuid, getppid};
    let args = std::env::args_os().skip(2).collect::<Vec<_>>();
    if args.len() != 4 || !geteuid().is_root() {
        return Err(failure("Invalid disk helper invocation."));
    }
    let number = |i: usize| {
        args[i]
            .to_str()
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| failure("Invalid disk helper identity."))
    };
    let parent = number(3)?;
    let uid: u32 = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|uid| *uid != 0)
        .ok_or_else(|| failure("Missing desktop polkit identity."))?;
    if getppid().map(|pid| pid.as_raw_nonzero().get() as u64) != Some(parent) {
        return Err(failure("Disk helper parent changed."));
    }
    let status = std::fs::read_to_string(format!("/proc/{parent}/status")).map_err(wire)?;
    let ids = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or_else(|| failure("Missing caller identity."))?
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(wire)?;
    if ids != [uid; 4] {
        return Err(failure("Polkit caller identity does not match."));
    }
    let disk = crate::Disk::new(&args[0])?;
    if disk.kind() != crate::DiskKind::Device || disk.identity() != (number(1)?, number(2)?) {
        return Err(failure(
            "The selected physical disk changed. Select it again.",
        ));
    }
    Ok(disk)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_are_bounded_and_paths_are_validated() {
        let mut oversized = ((REQUEST + 1) as u32).to_le_bytes().as_slice().to_vec();
        assert!(receive::<Request>(&mut oversized.as_slice(), REQUEST).is_err());
        oversized.clear();
        assert!(
            Path {
                face: 0,
                text: "../escape".into()
            }
            .checked()
            .is_err()
        );
        assert!(
            Path {
                face: 255,
                text: "file".into()
            }
            .checked()
            .is_err()
        );
        assert!(serde_json::from_str::<Request>(r#"{"Install":"anything"}"#).is_err());
    }
    #[test]
    fn broker_reads_and_streams_without_host_file_authority() {
        use std::os::unix::net::UnixStream;
        let dir = tempfile::tempdir().unwrap();
        let disk = crate::Disk::create_image(dir.path().join("disk.raw"), 64 << 20).unwrap();
        disk.format().unwrap();
        let source = dir.path().join("sample");
        let bytes = vec![b'x'; CHUNK * 3 + 7];
        std::fs::write(&source, &bytes).unwrap();
        let path = VolPath::home(0).unwrap().join("sample").unwrap();
        disk.install(&source, &path, false).unwrap();
        let browser = Browser::open(&disk).unwrap();
        let (left, mut right) = UnixStream::pair().unwrap();
        let writer = right.try_clone().unwrap();
        let worker = thread::spawn(move || serve(&mut right, writer, browser).unwrap());
        let mut client = Client::new(left.try_clone().unwrap(), left, None).unwrap();
        assert!(matches!(client.next().unwrap(), Response::Ready(_)));
        let remote = Remote(Mutex::new(client));
        assert_eq!(remote.head(&path, 10).unwrap(), vec![b'x'; 10]);
        assert!(remote.head(&path, CHUNK + 1).is_err());
        assert!(matches!(remote.read(&path, 5), Err(Error::TooLarge { .. })));
        assert_eq!(remote.read(&path, bytes.len() as u64).unwrap(), bytes);
        let (_, digest, copy) = remote.copy_to(&path, Vec::new()).unwrap();
        assert_eq!(digest, blake3::hash(&bytes));
        assert_eq!(copy, bytes);
        assert_eq!(remote.walk(&VolPath::home(0).unwrap()).unwrap().len(), 1);
        remote.close().unwrap();
        worker.join().unwrap();
        Browser::open(&disk).unwrap(); // close released both claim and engine lock
    }
}
