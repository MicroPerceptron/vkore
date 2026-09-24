//! Bounded discovery over explicitly authorized host directory capabilities.
//! Results are inert candidates; only Kore's later grants expose a text view.
use cap_std::{ambient_authority, fs::Dir};
use kore_workspace_sync::CanonicalPath;
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};
use workspacectl::SelectedFile;

const FILE_BYTES: u64 = 32 * 1024 * 1024;
const TEXT_BYTES: usize = 128 * 1024;
const RESULTS: usize = 8;
const ENTRIES: usize = 10_000;

pub(crate) struct SearchResult {
    pub files: Vec<SelectedFile>,
    pub identities: Vec<(String, PathBuf)>,
    pub scanned: usize,
    pub bounded: bool,
}

impl SearchResult {
    /// A matching file in the explicitly selected project is already offered
    /// by that root. Keep its stable path and the project's access policy;
    /// replacing it with a personal-search alias breaks subsequent reads and
    /// silently removes authorized writeback. External-only results still use
    /// the sparse read-only attachment.
    pub(crate) fn prefer_focus_root(&mut self, root: &Path) -> Result<bool, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let matched = self.files.iter().any(|file| {
            file.backing_path
                .canonicalize()
                .is_ok_and(|path| path.starts_with(&root))
        });
        if matched {
            self.files.clear();
            self.identities.clear();
        }
        Ok(matched)
    }
}

struct Candidate {
    score: usize,
    modified: u64,
    file: SelectedFile,
}

pub(crate) fn search(query: &str, locations: &[PathBuf]) -> Result<SearchResult, String> {
    let words: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .filter(|w| {
            w.len() >= 3
                && !matches!(
                    w.as_str(),
                    "the"
                        | "what"
                        | "was"
                        | "from"
                        | "last"
                        | "week"
                        | "and"
                        | "file"
                        | "read"
                        | "report"
                        | "contents"
                        | "that"
                        | "this"
                        | "with"
                        | "please"
                        | "show"
                        | "have"
                        | "its"
                        | "txt"
                        | "md"
                        | "csv"
                        | "json"
                        | "pptx"
                        | "docx"
                        | "again"
                )
        })
        .collect();
    if words.is_empty() {
        return Ok(SearchResult {
            files: vec![],
            identities: vec![],
            scanned: 0,
            bounded: false,
        });
    }
    let presentation =
        query.to_lowercase().contains("slide") || query.to_lowercase().contains("presentation");
    let recent = query.to_lowercase().contains("last week");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut scanned = 0;
    let mut bounded = locations.len() > 16;
    for location in locations.iter().take(16) {
        let root = Dir::open_ambient_dir(location, ambient_authority())
            .map_err(|e| format!("Authorized search location {}: {e}", location.display()))?;
        let mut pending = vec![(root, PathBuf::new(), 0u8)];
        while let Some((dir, relative, depth)) = pending.pop() {
            let entries = match dir.entries() {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries {
                if scanned >= ENTRIES || Instant::now() >= deadline {
                    bounded = true;
                    break;
                }
                scanned += 1;
                let Ok(entry) = entry else { continue };
                let name = entry.file_name();
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_symlink() {
                    continue;
                }
                let path = relative.join(&name);
                if kind.is_dir() {
                    if depth < 12
                        && pending.len() < 256
                        && let Ok(child) = entry.open_dir()
                    {
                        pending.push((child, path, depth + 1));
                    } else {
                        bounded = true;
                    }
                    continue;
                }
                if !kind.is_file() {
                    continue;
                }
                let extension = Path::new(&name)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if !matches!(
                    extension.as_str(),
                    "txt" | "md" | "csv" | "json" | "rs" | "py" | "html" | "pptx" | "docx"
                ) {
                    continue;
                }
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.len() > FILE_BYTES {
                    continue;
                }
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.into_std().duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                let Ok(mut file) = entry.open() else { continue };
                let mut bytes = Vec::new();
                if file
                    .by_ref()
                    .take(FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .is_err()
                    || bytes.len() as u64 > FILE_BYTES
                {
                    continue;
                }
                let Ok(text) = text_content(&extension, &bytes) else {
                    continue;
                };
                let searchable = format!("{}\n{}", path.display(), text).to_lowercase();
                let mut score = words
                    .iter()
                    .filter(|word| searchable.contains(word.as_str()))
                    .count();
                if presentation && extension == "pptx" {
                    score += 3;
                }
                if score == 0 {
                    continue;
                }
                if recent && now.saturating_sub(modified) <= 14 * 86400 {
                    score += 2;
                }
                let backing = location.join(&path);
                let digest = blake3::hash(backing.as_os_str().as_encoded_bytes()).to_hex();
                let stem: String = Path::new(&name)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .take(17)
                    .collect();
                let alias = format!(
                    "{}-{}.txt",
                    if stem.is_empty() { "document" } else { &stem },
                    &digest[..8]
                );
                let Ok(alias) = CanonicalPath::new(alias.as_bytes()) else {
                    continue;
                };
                let Ok(parent) = dir.try_clone() else {
                    continue;
                };
                let Ok(selected) = SelectedFile::new(
                    parent,
                    PathBuf::from(name),
                    alias,
                    backing,
                    *blake3::hash(&bytes).as_bytes(),
                    text.into_bytes(),
                ) else {
                    continue;
                };
                candidates.push(Candidate {
                    score,
                    modified,
                    file: selected,
                });
                candidates.sort_by(|a, b| {
                    b.score
                        .cmp(&a.score)
                        .then(b.modified.cmp(&a.modified))
                        .then(a.file.backing_path.cmp(&b.file.backing_path))
                });
                candidates.truncate(RESULTS);
            }
            if scanned >= ENTRIES || Instant::now() >= deadline {
                break;
            }
        }
        if scanned >= ENTRIES || Instant::now() >= deadline {
            break;
        }
    }
    let files: Vec<_> = candidates
        .into_iter()
        .map(|candidate| candidate.file)
        .collect();
    let identities = files
        .iter()
        .map(|file| (file.alias.as_str().to_owned(), file.backing_path.clone()))
        .collect();
    Ok(SearchResult {
        files,
        identities,
        scanned,
        bounded,
    })
}

fn text_content(extension: &str, bytes: &[u8]) -> Result<String, String> {
    if !matches!(extension, "pptx" | "docx") {
        let text = std::str::from_utf8(bytes).map_err(|_| "Document is not UTF-8")?;
        if text.len() > TEXT_BYTES {
            return Err("Text exceeds extraction bound".into());
        }
        return Ok(text.to_owned());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if archive.len() > 2048 {
        return Err("Document has too many members".into());
    }
    let mut names: Vec<String> = archive
        .file_names()
        .filter(|name| {
            (extension == "pptx" && name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
                || (extension == "docx" && *name == "word/document.xml")
        })
        .map(str::to_owned)
        .collect();
    names.sort_by_key(|name| {
        name.strip_prefix("ppt/slides/slide")
            .and_then(|s| s.strip_suffix(".xml"))
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
    });
    let mut text = String::new();
    let mut extracted_bytes = 0usize;
    for name in names {
        let mut member = archive.by_name(&name).map_err(|e| e.to_string())?;
        if member.size() > 2 * 1024 * 1024 {
            return Err("Document member exceeds bound".into());
        }
        let mut xml = String::new();
        member
            .by_ref()
            .take(2 * 1024 * 1024 + 1)
            .read_to_string(&mut xml)
            .map_err(|e| e.to_string())?;
        if xml.len() > 2 * 1024 * 1024 {
            return Err("Document member exceeds bound".into());
        }
        extracted_bytes += xml.len();
        if extracted_bytes > 8 * 1024 * 1024 {
            return Err("Document expansion exceeds bound".into());
        }
        let document = roxmltree::Document::parse(&xml).map_err(|e| e.to_string())?;
        text.push_str(&format!("[{name}]\n"));
        for node in document.descendants().filter(|node| node.has_tag_name("t")) {
            if let Some(value) = node.text() {
                if text.len() + value.len() + 1 > TEXT_BYTES {
                    return Err("Document text exceeds bound".into());
                }
                text.push_str(value);
                text.push('\n');
            }
        }
    }
    if text.is_empty() {
        return Err("No readable document text".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn matching_project_files_keep_the_focus_root_instead_of_search_aliases() {
        let parent = tempfile::tempdir().unwrap();
        let focus = parent.path().join("project");
        let sibling = parent.path().join("project-other");
        std::fs::create_dir(&focus).unwrap();
        std::fs::create_dir(&sibling).unwrap();
        std::fs::write(focus.join("test.txt"), "hello").unwrap();
        std::fs::write(sibling.join("external.txt"), "outside").unwrap();
        let roots = [focus.clone(), sibling.clone()];
        let mut local = search("Read test.txt and reply with its exact contents.", &roots).unwrap();
        assert_eq!(local.files.len(), 1);
        assert!(local.prefer_focus_root(&focus).unwrap());
        assert!(local.files.is_empty());
        assert!(local.identities.is_empty());
        let mut external = search("Read external.txt", &roots).unwrap();
        assert!(!external.prefer_focus_root(&focus).unwrap());
        assert_eq!(external.files.len(), 1);
        assert_eq!(external.identities[0].1, sibling.join("external.txt"));
        let mut mixed = search("Read test.txt and external.txt", &roots).unwrap();
        assert!(mixed.prefer_focus_root(&focus).unwrap());
        assert!(
            mixed.files.is_empty(),
            "explicit project match keeps the selected root"
        );
    }

    #[test]
    fn file_extensions_and_generic_request_words_do_not_select_unrelated_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("unrelated.txt"),
            "This is a separate document about gardening.",
        )
        .unwrap();
        let found = search(
            "Read acceptance.txt and report its contents.",
            &[root.path().to_owned()],
        )
        .unwrap();
        assert!(found.files.is_empty());
    }

    #[test]
    fn discovery_stays_inside_authorized_locations_and_distinguishes_names() {
        let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        for (index, root) in roots.iter().enumerate() {
            std::fs::write(
                root.path().join("growth.txt"),
                format!("Revenue growth {}%", index + 17),
            )
            .unwrap();
        }
        let first = search("revenue growth", &[roots[0].path().into()]).unwrap();
        assert_eq!(first.files.len(), 1);
        assert!(first.identities[0].1.starts_with(roots[0].path()));
        let both = search(
            "revenue growth",
            &roots.iter().map(|r| r.path().into()).collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(both.files.len(), 2);
        assert_ne!(both.identities[0].0, both.identities[1].0);
        assert!(
            search("unrelated", &[roots[0].path().into()])
                .unwrap()
                .files
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_do_not_expand_discovery_authority() {
        let allowed = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("secret.txt"), "growth 900%").unwrap();
        std::os::unix::fs::symlink(other.path(), allowed.path().join("outside")).unwrap();
        std::os::unix::fs::symlink(
            other.path().join("secret.txt"),
            allowed.path().join("growth.txt"),
        )
        .unwrap();
        assert!(
            search("growth", &[allowed.path().into()])
                .unwrap()
                .files
                .is_empty()
        );
    }

    #[test]
    fn presentation_text_uses_actual_slide_contents_and_numeric_order() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (slide, value) in [(10, "Conclusion"), (2, "Growth was 17.8%")] {
            archive
                .start_file(
                    format!("ppt/slides/slide{slide}.xml"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            write!(
                archive,
                "<p:sld xmlns:p=\"p\" xmlns:a=\"a\"><a:t>{value}</a:t></p:sld>"
            )
            .unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        let text = text_content("pptx", &bytes).unwrap();
        assert!(text.contains("Growth was 17.8%"));
        assert!(text.find("slide2.xml").unwrap() < text.find("slide10.xml").unwrap());
        assert!(text_content("pptx", b"not a presentation").is_err());
    }
}
