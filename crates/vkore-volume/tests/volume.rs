//! vKore's volume layer over Kore's own engine, end to end on real images:
//! what a browser sees, what an export writes on the host, and a project's
//! round trip through a terminal's home.

#![cfg(unix)]

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use vkore_volume::{
    Browser, Disk, EntryKind, Error, ExportOptions, Listing, Preview, VolPath, preview,
    workspace::{self, ApplyLimits, StageOptions},
};

const MIB: u64 = 1 << 20;

fn formatted(dir: &Path) -> Disk {
    let disk = Disk::create_image(dir.join("disk.raw"), 64 * MIB).unwrap();
    disk.format().unwrap();
    disk
}

fn tree(root: &Path, files: &[(&str, &[u8])]) {
    for (path, bytes) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

fn host_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, at: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                out.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn home(path: &str) -> VolPath {
    VolPath::parse(0, path).unwrap()
}

#[test]
fn a_browser_sees_what_an_install_put_there_and_holds_the_disk_while_open() {
    let dir = tempfile::tempdir().unwrap();
    let disk = Disk::create_image(dir.path().join("disk.raw"), 64 * MIB).unwrap();
    assert!(matches!(Browser::open(&disk), Err(error) if error.is_blank()));
    disk.format().unwrap();
    let browser = Browser::open(&disk).unwrap();
    assert_eq!(browser.list(&home("")).unwrap(), Listing::EmptyHome);
    // One owner per disk in this process: a second browser or a write
    // while this one reads would drop its lock when it closed.
    assert!(
        matches!(Browser::open(&disk), Err(Error::Path(message)) if message.contains("already open"))
    );
    let source = dir.path().join("src");
    tree(
        &source,
        &[
            ("a.txt", b"alpha"),
            ("sub/b.txt", b"beta"),
            ("sub/deeper/c.bin", &[7; 70_000]),
        ],
    );
    assert!(disk.install(&source, &home("proj"), false).is_err());
    browser.close().unwrap();

    let installed = disk.install(&source, &home("proj"), false).unwrap();
    assert_eq!((installed.files, installed.directories), (3, 3));
    let browser = Browser::open(&disk).unwrap();
    assert_eq!(browser.status().generation, installed.generation);
    let Listing::Directory { entries } = browser.list(&home("proj")).unwrap() else {
        panic!("proj is a directory");
    };
    // Directories first, each group by name.
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.kind))
            .collect::<Vec<_>>(),
        [("sub", EntryKind::Directory), ("a.txt", EntryKind::File)]
    );
    assert_eq!(
        browser.list(&home("proj/a.txt")).unwrap(),
        Listing::File { len: 5 }
    );
    assert_eq!(browser.read(&home("proj/sub/b.txt"), 4).unwrap(), b"beta");
    assert!(matches!(
        browser.read(&home("proj/sub/deeper/c.bin"), 1024),
        Err(Error::TooLarge { limit: 1024, .. })
    ));
    let walked = browser.walk(&home("")).unwrap();
    let paths = walked
        .iter()
        .map(|entry| (entry.path.as_str(), entry.depth))
        .collect::<Vec<_>>();
    assert!(paths.contains(&("proj", 1)));
    assert!(paths.contains(&("proj/sub/deeper/c.bin", 4)));
    let root = browser.list(&VolPath::root()).unwrap();
    assert!(
        matches!(root, Listing::Directory { entries } if entries.iter().any(|e| e.name == "work"))
    );
    drop(browser);

    disk.remove(&home("proj/sub")).unwrap();
    let browser = Browser::open(&disk).unwrap();
    assert!(matches!(
        browser.list(&home("proj/sub")),
        Err(Error::Volume(_))
    ));
}

#[test]
fn an_export_writes_inside_its_destination_and_never_over_what_is_there_unasked() {
    let dir = tempfile::tempdir().unwrap();
    let disk = formatted(dir.path());
    let source = dir.path().join("src");
    tree(
        &source,
        &[
            ("shot.png", b"\x89PNG not really"),
            ("logs/boot.log", b"boot: ok\n"),
            ("logs/old/0.log", b""),
        ],
    );
    disk.install(&source, &home("pictures"), false).unwrap();
    let browser = Browser::open(&disk).unwrap();
    let out = dir.path().join("out");
    fs::create_dir(&out).unwrap();

    let file = browser
        .export(&home("pictures/shot.png"), &out, ExportOptions::default())
        .unwrap();
    assert_eq!((file.files, file.bytes), (1, 15));
    assert_eq!(
        fs::read(out.join("shot.png")).unwrap(),
        b"\x89PNG not really"
    );
    fs::write(out.join("shot.png"), b"kept").unwrap();
    assert!(matches!(
        browser.export(&home("pictures/shot.png"), &out, ExportOptions::default()),
        Err(Error::Exists(_))
    ));
    assert_eq!(fs::read(out.join("shot.png")).unwrap(), b"kept");
    browser
        .export(
            &home("pictures/shot.png"),
            &out,
            ExportOptions { replace: true },
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("shot.png")).unwrap(),
        b"\x89PNG not really"
    );

    let tree_out = browser
        .export(&home("pictures"), &out, ExportOptions::default())
        .unwrap();
    assert_eq!((tree_out.files, tree_out.directories), (3, 3));
    assert_eq!(host_files(&out.join("pictures")), host_files(&source));
    // No partial file is left behind.
    assert!(fs::read_dir(out.join("pictures")).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".partial")
    }));

    // A link in the destination is never written through.
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    let linked = dir.path().join("linked");
    fs::create_dir(&linked).unwrap();
    symlink(&elsewhere, linked.join("logs")).unwrap();
    assert!(
        browser
            .export(&home("pictures/logs"), &linked, ExportOptions::default())
            .is_err()
    );
    assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
}

#[test]
fn a_project_round_trips_through_a_terminal_s_home() {
    let dir = tempfile::tempdir().unwrap();
    let disk = Disk::create_image(dir.path().join("disk.raw"), 64 * MIB).unwrap();
    let project = dir.path().join("project");
    tree(
        &project,
        &[
            ("README.md", b"# demo\n"),
            ("src/main.rs", b"fn main() {}\n"),
            ("src/old.rs", b"// gone soon\n"),
            (
                "docs/a-name-much-longer-than-thirty-two-bytes.md",
                b"skipped",
            ),
            (".git/HEAD", b"ref: refs/heads/main\n"),
        ],
    );
    symlink("/etc/hostname", project.join("hostname")).unwrap();
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();

    // A blank image is formatted as the machine's first boot would.
    let manifest = workspace::stage(&disk, &project, &scratch, &StageOptions::new("demo")).unwrap();
    assert_eq!(
        manifest
            .files
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["README.md", "src/main.rs", "src/old.rs"]
    );
    let skipped = manifest
        .skipped
        .iter()
        .map(|skip| skip.path.as_str())
        .collect::<Vec<_>>();
    assert!(skipped.contains(&".git"));
    assert!(skipped.contains(&"hostname"));
    assert!(skipped.contains(&"docs/a-name-much-longer-than-thirty-two-bytes.md"));
    let record = scratch.join("demo.json");
    manifest.save(&record).unwrap();
    assert_eq!(workspace::Manifest::load(&record).unwrap(), manifest);

    // What a machine might do in terminal 0: edit, create, delete.
    let edits = dir.path().join("edits");
    tree(
        &edits,
        &[
            ("main.rs", b"fn main() { println!(\"kore\"); }\n"),
            ("notes/todo.txt", b"ship it\n"),
        ],
    );
    disk.install(&edits.join("main.rs"), &home("demo/src/main.rs"), true)
        .unwrap();
    disk.install(&edits.join("notes"), &home("demo/notes"), false)
        .unwrap();
    disk.remove(&home("demo/src/old.rs")).unwrap();

    let changes = workspace::collect(&disk, &manifest, &scratch).unwrap();
    assert_eq!(
        changes
            .modified
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        ["src/main.rs"]
    );
    assert_eq!(
        changes
            .created
            .iter()
            .map(|c| c.path.as_str())
            .collect::<Vec<_>>(),
        ["notes/todo.txt"]
    );
    assert_eq!(changes.deleted, ["src/old.rs"]);
    assert_eq!(changes.created_directories, ["notes"]);
    assert_eq!(changes.file_count(), 3);

    // Beyond the caller's bound: refused whole.
    let tight = ApplyLimits {
        max_files: 2,
        max_bytes: 1 << 20,
    };
    assert!(matches!(
        workspace::apply(&changes, &manifest, tight),
        Err(Error::Bounds(_))
    ));
    assert_eq!(
        fs::read(project.join("src/main.rs")).unwrap(),
        b"fn main() {}\n"
    );

    // A host edit since staging: refused whole, nothing written.
    fs::write(
        project.join("src/old.rs"),
        b"// edited on the host meanwhile\n",
    )
    .unwrap();
    let limits = ApplyLimits {
        max_files: 16,
        max_bytes: 1 << 20,
    };
    assert!(matches!(
        workspace::apply(&changes, &manifest, limits),
        Err(Error::Conflict(paths)) if paths == ["src/old.rs"]
    ));
    assert_eq!(
        fs::read(project.join("src/main.rs")).unwrap(),
        b"fn main() {}\n"
    );
    assert!(!project.join("notes").exists());

    fs::write(project.join("src/old.rs"), b"// gone soon\n").unwrap();
    let applied = workspace::apply(&changes, &manifest, limits).unwrap();
    assert_eq!(
        (
            applied.written,
            applied.deleted,
            applied.directories_created
        ),
        (2, 1, 1)
    );
    assert_eq!(
        fs::read(project.join("src/main.rs")).unwrap(),
        b"fn main() { println!(\"kore\"); }\n"
    );
    assert_eq!(
        fs::read(project.join("notes/todo.txt")).unwrap(),
        b"ship it\n"
    );
    assert!(!project.join("src/old.rs").exists());
    // What was never staged is never touched.
    assert_eq!(
        fs::read(project.join(".git/HEAD")).unwrap(),
        b"ref: refs/heads/main\n"
    );
    assert!(
        fs::symlink_metadata(project.join("hostname"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn a_project_too_wide_for_a_kore_directory_is_refused_before_anything_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let disk = formatted(dir.path());
    let project = dir.path().join("wide");
    fs::create_dir(&project).unwrap();
    for index in 0..129 {
        fs::write(project.join(format!("{index}")), b"").unwrap();
    }
    let scratch = dir.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let refused = workspace::stage(&disk, &project, &scratch, &StageOptions::new("wide"));
    assert!(matches!(refused, Err(Error::Bounds(message)) if message.contains("at most 128")));
    let browser = Browser::open(&disk).unwrap();
    assert_eq!(browser.list(&home("")).unwrap(), Listing::EmptyHome);
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
}

#[test]
fn a_screenshot_on_a_volume_previews_with_its_capture_record() {
    let dir = tempfile::tempdir().unwrap();
    let disk = formatted(dir.path());
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 2, 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .add_text_chunk("Software".into(), "kore 1b6cb32271d3".into())
            .unwrap();
        encoder
            .add_text_chunk("kore:kernel-build".into(), "1b6cb32271d3".into())
            .unwrap();
        encoder
            .add_text_chunk("kore:mode".into(), "screen".into())
            .unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0; 12]).unwrap();
        writer.finish().unwrap();
    }
    let shots = dir.path().join("screenshots");
    fs::create_dir(&shots).unwrap();
    fs::write(shots.join("2026-11-02_11-03-14.png"), &png).unwrap();
    disk.install(&shots, &home(vkore_volume::SCREENSHOTS), false)
        .unwrap();
    let browser = Browser::open(&disk).unwrap();
    let path = home("pictures/screenshots/2026-11-02_11-03-14.png");
    let bytes = browser.head(&path, preview::PREVIEW_HEAD_BYTES).unwrap();
    let Preview::Image { width, capture, .. } = preview::preview(&bytes, png.len() as u64) else {
        panic!("a screenshot previews as an image");
    };
    assert_eq!(width, 2);
    assert_eq!(
        capture.unwrap().kernel_build.as_deref(),
        Some("1b6cb32271d3")
    );
    let _: PathBuf = disk.path().to_path_buf();
}

#[test]
fn a_head_reads_no_further_and_an_open_browser_outlasts_any_number_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let disk = formatted(dir.path());
    let source = dir.path().join("log");
    let log = (0..200_000u32)
        .map(|at| b"kore\n"[at as usize % 5])
        .collect::<Vec<_>>();
    fs::write(&source, &log).unwrap();
    disk.install(&source, &home("logs/boot.log"), false)
        .unwrap();
    let browser = Browser::open(&disk).unwrap();
    let path = home("logs/boot.log");
    // A session holds 32 engine handles. Each head stops a copy part way,
    // and each miss or wrong kind is refused: none may keep what it opened.
    for _ in 0..100 {
        assert_eq!(browser.head(&path, 4096).unwrap(), &log[..4096]);
        assert!(browser.head(&home("logs/missing.log"), 4096).is_err());
        assert!(browser.head(&home("logs"), 4096).is_err());
        assert!(matches!(
            browser.read(&path, 1024),
            Err(Error::TooLarge { .. })
        ));
    }
    // A head as long as the file, or longer, is the file.
    assert_eq!(browser.head(&path, log.len()).unwrap(), log);
    assert_eq!(browser.head(&path, log.len() + 1).unwrap(), log);
    let Listing::Directory { entries } = browser.list(&home("logs")).unwrap() else {
        panic!("logs is a directory");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(browser.read(&path, 1 << 20).unwrap(), log);
}
