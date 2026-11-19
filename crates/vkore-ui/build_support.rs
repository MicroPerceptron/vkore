//! Build identity is display metadata; Cargo's release version stays SemVer.
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub struct BuildInfo {
    pub revision: String,
    pub watch: Vec<PathBuf>,
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_owned()
    })
}

pub fn inspect(root: &Path, supplied: Option<&str>) -> Result<BuildInfo, String> {
    let valid = |s: &str| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
    if supplied.is_some_and(|s| !valid(s)) {
        return Err("VKORE_BUILD_COMMIT must be a full hexadecimal Git commit ID".into());
    }
    // A source archive nested in another repository must not inherit its hash.
    let own_repo = git(root, &["rev-parse", "--show-toplevel"])
        .and_then(|p| std::fs::canonicalize(p).ok())
        == std::fs::canonicalize(root).ok();
    let mut watch = Vec::new();
    if root.join(".git").exists() {
        watch.push(root.join(".git"));
    }
    let revision = if own_repo {
        let head = git(root, &["rev-parse", "HEAD"])
            .filter(|s| valid(s))
            .ok_or("cannot read build commit")?;
        if supplied.is_some_and(|s| !s.eq_ignore_ascii_case(&head)) {
            return Err("VKORE_BUILD_COMMIT disagrees with the checkout".into());
        }
        let mut refs = vec![
            "HEAD".to_owned(),
            "packed-refs".to_owned(),
            "index".to_owned(),
        ];
        if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
            refs.push(reference);
        }
        for reference in refs {
            if let Some(path) = git(
                root,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    &reference,
                ],
            ) {
                // Cargo continually rebuilds for nonexistent paths. Their parent
                // catches creation (e.g. a ref packed by git gc).
                let mut path = PathBuf::from(path);
                while !path.exists() && path.pop() {}
                watch.push(path);
            }
        }
        if let Some(files) = git(root, &["ls-files", "-z"]) {
            watch.extend(
                files
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(|p| root.join(p)),
            );
        }
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["diff", "--quiet", "HEAD", "--"])
            .status()
            .map_err(|e| e.to_string())?;
        match status.code() {
            Some(0) => head[..7].to_owned(),
            Some(1) => format!("{}-dirty", &head[..7]),
            _ => return Err("cannot determine whether build sources are modified".into()),
        }
    } else {
        supplied
            .map(|s| s[..7].to_ascii_lowercase())
            .unwrap_or_else(|| "unknown".into())
    };
    Ok(BuildInfo { revision, watch })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(root: &Path, args: &[&str]) {
        assert!(git(root, args).is_some(), "git {args:?}");
    }
    #[test]
    fn tracks_commits_dirty_sources_and_linked_worktrees() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        std::fs::create_dir(&root).unwrap();
        run(&root, &["init", "-q"]);
        run(&root, &["config", "user.email", "test@example.invalid"]);
        run(&root, &["config", "user.name", "Test"]);
        std::fs::write(root.join("source"), "one").unwrap();
        run(&root, &["add", "source"]);
        run(&root, &["commit", "-qm", "initial"]);
        let first = inspect(&root, None).unwrap();
        assert_eq!(first.revision.len(), 7);
        assert!(first.watch.iter().any(|p| p.ends_with("HEAD")));
        std::fs::write(root.join("untracked"), "ignored").unwrap();
        assert_eq!(inspect(&root, None).unwrap().revision, first.revision);
        std::fs::write(root.join("source"), "two").unwrap();
        assert_eq!(
            inspect(&root, None).unwrap().revision,
            format!("{}-dirty", first.revision)
        );
        run(&root, &["commit", "-qam", "change"]);
        let head = git(&root, &["rev-parse", "HEAD"]).unwrap();
        assert_ne!(inspect(&root, None).unwrap().revision, first.revision);
        assert_eq!(inspect(&root, Some(&head)).unwrap().revision, head[..7]);
        assert!(inspect(&root, Some(&"0".repeat(40))).is_err());
        let linked = temp.path().join("linked");
        run(
            &root,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let info = inspect(&linked, None).unwrap();
        assert_eq!(info.revision, head[..7]);
        assert!(
            info.watch
                .iter()
                .any(|p| p.ends_with("worktrees/linked/HEAD"))
        );
        let archive = root.join("archive");
        std::fs::create_dir(&archive).unwrap();
        assert_eq!(inspect(&archive, None).unwrap().revision, "unknown");
        assert_eq!(inspect(&archive, Some(&head)).unwrap().revision, head[..7]);
        assert!(inspect(&archive, Some("abc\ncargo:bad")).is_err());
    }
}
