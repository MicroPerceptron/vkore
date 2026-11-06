//! How far a Kore boot has come, read from its serial console.
//!
//! The console is CRLF, carries the firmware's ANSI escapes before the
//! kernel starts, and is written by asynchronous lanes, so lines arrive
//! late and out of order: the parser is incremental, keeps a partial line
//! across reads, and decides from which witnesses have appeared, never from
//! their order. The witnesses are the ones Kore's production boot gate
//! asserts (`Makefile`, `run-boot-gate-x86`):
//!
//! | Line | Meaning |
//! | --- | --- |
//! | `kore <version>: console up` | the kernel runs |
//! | `kore: build <stamp>` | the exact build (painted at the first boot phase) |
//! | `first-userland-boot: services founded (N/M)` | the service rows stand |
//! | `first-shell: ready` | the desktop's shell is usable |
//! | `app-store: seeded N/M … libs: L/L data: D/D` | applications are installed |
//! | `home-skeleton: homes: P/A copied: C fail: S` | the terminals' homes exist |
//!
//! and fail on `kore: BOOT DEGRADED`, a failed service row, `first-shell:
//! FAIL`, a failed bootstrap testimony, or a kernel panic or fault. Another
//! witness's `FAIL` is recorded as a warning: the machine still serves.

use serde::Serialize;

/// The furthest a boot has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Firmware and loader: nothing from the kernel yet.
    Firmware,
    /// The kernel's console is up.
    Kernel,
    /// Every service row founded and ready.
    Services,
    /// The shell answers: the desktop is usable.
    Shell,
    /// Applications are seeded and the terminals' homes exist.
    Ready,
    /// Booted, but a row the system needs did not come up.
    Degraded,
    /// The boot failed: a kernel panic or fault, or a refused founding.
    Failed,
}

/// A boot's progress, as its serial witnesses tell it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Readiness {
    /// `0.40.0` from `kore 0.40.0: console up`.
    pub version: Option<String>,
    /// The kernel's build stamp, when it painted one.
    pub build: Option<String>,
    /// `profile=4k48 …` from the kernel's paging line.
    pub paging: Option<String>,
    /// The framebuffer mode the kernel claimed.
    pub display: Option<(u32, u32)>,
    pub services: Option<(u32, u32)>,
    pub shell: bool,
    pub apps: Option<(u32, u32)>,
    pub homes: Option<(u32, u32)>,
    /// `init-phase: <name> at <ms>ms`, as they arrive.
    pub phases: Vec<(String, u64)>,
    /// Lines that ended or degraded the boot.
    pub failures: Vec<String>,
    /// Witnesses that reported `FAIL` while the machine still serves.
    pub warnings: Vec<String>,
    /// Lines seen.
    pub lines: u64,
    #[serde(skip)]
    partial: String,
    #[serde(skip)]
    degraded: bool,
    #[serde(skip)]
    failed: bool,
}

impl Readiness {
    pub fn new() -> Self {
        Self::default()
    }

    /// The furthest stage the witnesses so far establish.
    pub fn stage(&self) -> Stage {
        if self.failed {
            Stage::Failed
        } else if self.degraded {
            Stage::Degraded
        } else if self.shell && self.apps.is_some() && self.homes.is_some() {
            Stage::Ready
        } else if self.shell {
            Stage::Shell
        } else if self.services.is_some_and(|(founded, of)| founded == of) {
            Stage::Services
        } else if self.version.is_some() {
            Stage::Kernel
        } else {
            Stage::Firmware
        }
    }

    /// Feed bytes as the console wrote them; answer the stage changes they
    /// caused, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Stage> {
        let mut changes = Vec::new();
        let mut stage = self.stage();
        self.partial.push_str(&String::from_utf8_lossy(bytes));
        while let Some(end) = self.partial.find('\n') {
            let line = strip(&self.partial[..end]);
            self.partial.drain(..=end);
            self.line(&line);
            let now = self.stage();
            if now != stage {
                changes.push(now);
                stage = now;
            }
        }
        changes
    }

    fn line(&mut self, line: &str) {
        self.lines += 1;
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("kore ")
            && let Some((version, _)) = rest.split_once(": console up")
        {
            self.version = Some(version.to_owned());
            return;
        }
        if let Some(stamp) = line.strip_prefix("kore: build ") {
            self.build = Some(stamp.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("kore: paging — ") {
            self.paging = Some(rest.to_owned());
        } else if let Some(rest) = line.strip_prefix("kore: display aperture claimed ") {
            self.display = rest
                .split_whitespace()
                .next()
                .and_then(|mode| mode.split_once('x'))
                .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)));
        } else if line.starts_with("kore: BOOT DEGRADED") {
            self.degraded = true;
            self.failures.push(line.to_owned());
        } else if let Some(rest) = line.strip_prefix("first-userland-boot: services founded (") {
            self.services = fraction(rest);
        } else if line.starts_with("first-shell: ready") {
            self.shell = true;
        } else if let Some(rest) = line.strip_prefix("app-store: seeded ") {
            self.apps = fraction(rest);
        } else if let Some(rest) = line.strip_prefix("home-skeleton: homes: ") {
            self.homes = fraction(rest);
        } else if let Some(rest) = line.strip_prefix("init-phase: ") {
            if let Some((phase, at)) = rest.split_once(" at ")
                && let Ok(ms) = at.trim_end_matches("ms").parse()
            {
                self.phases.push((phase.to_owned(), ms));
            }
        } else if fatal(line) {
            self.failed = true;
            self.failures.push(line.to_owned());
        } else if line.contains(": FAIL") {
            self.warnings.push(line.to_owned());
        }
    }
}

/// Lines that end a boot.
fn fatal(line: &str) -> bool {
    line.starts_with("error[kernel-panic]")
        || line.starts_with("error[kernel-fault]")
        || line.starts_with("first-shell: FAIL")
        || line.starts_with("bootstrap-testimony: FAIL")
        || (line.starts_with("first-userland-boot:")
            && (line.contains("ready: FAIL")
                || line.contains("founded: FAIL")
                || line.contains("founding FAILED")))
}

/// `N/M` at the start of `text`.
fn fraction(text: &str) -> Option<(u32, u32)> {
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '/'))
        .unwrap_or(text.len());
    let (left, right) = text[..end].split_once('/')?;
    Some((left.parse().ok()?, right.parse().ok()?))
}

/// A console line without its carriage returns and terminal escapes.
fn strip(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {}
            '\u{1b}' => {
                // CSI: ESC [ parameters final-byte.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lines from a real v0.40.0 production boot under TCG.
    const BOOT: &str = "\x1b[2J\x1b[01;01H\x1b[=3h\x1b[2J\x1b[01;01HBdsDxe: loading Boot0001 \"UEFI QEMU DVD-ROM QM00005 \"\r\n\
\x1b[01;01H\x1b[01;01Hkore: _start\r\n\
kore 0.40.0: console up \u{2014} 1 serial sink(s)\r\n\
kore: display aperture claimed 1280x800 @ 0x81000000 (pitch 5120)\r\n\
kore: paging \u{2014} profile=4k48 granule=4K va=48 pa=52 live-pa=52 levels=4\r\n\
net-load: PASS (a full ring of unicast frames went out and every reply came back)\r\n\
init-phase: installed at 31035ms\r\n\
first-shell: ready (witness 0x5)\r\n\
first-userland-boot: services founded (19/19); PARENT_SEND beacons 19/19; kernel_heap peak 14947024B\r\n\
init-phase: ready at 43601ms\r\n\
app-store: seeded 44/44 tree-load: ok already-resident: ok tree-acquire: ok libs: 3/3 data: 15/15 pkg: ok lib-fail: 0\r\n\
home-skeleton: homes: 1/1 copied: 7 fail: 0\r\n";

    #[test]
    fn a_real_boot_reads_through_to_ready() {
        let mut readiness = Readiness::new();
        // Fed in uneven pieces, as a tail of the log file reads it.
        let mut changes = Vec::new();
        for piece in BOOT.as_bytes().chunks(37) {
            changes.extend(readiness.feed(piece));
        }
        assert_eq!(changes, [Stage::Kernel, Stage::Shell, Stage::Ready]);
        assert_eq!(readiness.version.as_deref(), Some("0.40.0"));
        assert_eq!(readiness.display, Some((1280, 800)));
        assert_eq!(readiness.services, Some((19, 19)));
        assert_eq!(readiness.apps, Some((44, 44)));
        assert_eq!(readiness.homes, Some((1, 1)));
        assert!(
            readiness
                .paging
                .as_deref()
                .unwrap()
                .starts_with("profile=4k48")
        );
        assert_eq!(
            readiness.phases,
            [("installed".into(), 31035), ("ready".into(), 43601)]
        );
        assert!(readiness.failures.is_empty() && readiness.warnings.is_empty());
    }

    #[test]
    fn failures_end_the_boot_and_witness_failures_only_warn() {
        let mut readiness = Readiness::new();
        readiness.feed(b"kore 0.40.0: console up\r\nnet-load: FAIL (no reply)\r\n");
        assert_eq!(readiness.stage(), Stage::Kernel);
        assert_eq!(readiness.warnings.len(), 1);
        assert_eq!(
            readiness.feed(b"error[kernel-panic]: Rust panic at src/x.rs:1\r\n"),
            [Stage::Failed]
        );
        let mut degraded = Readiness::new();
        degraded.feed(b"kore 0.40.0: console up\r\nkore: BOOT DEGRADED \xe2\x80\x94 services founded 17/19\r\n");
        assert_eq!(degraded.stage(), Stage::Degraded);
        let mut row = Readiness::new();
        row.feed(b"first-userland-boot: kore-svc-dagfs ready: FAIL (timeout)\n");
        assert_eq!(row.stage(), Stage::Failed);
        // `lib-fail: 0` is a count, not a failure.
        let mut fine = Readiness::new();
        fine.feed(b"app-store: seeded 44/44 lib-fail: 0\n");
        assert!(fine.failures.is_empty() && fine.warnings.is_empty());
    }

    #[test]
    fn partial_lines_wait_for_their_end() {
        let mut readiness = Readiness::new();
        assert!(readiness.feed(b"kore 0.40.0: cons").is_empty());
        assert_eq!(readiness.feed(b"ole up\r\n"), [Stage::Kernel]);
        assert_eq!(fraction("19/19); PARENT"), Some((19, 19)));
        assert_eq!(fraction("x/y"), None);
        assert_eq!(strip("\x1b[01;01Hkore\r"), "kore");
    }
}
