//! Adapter for the configured first-party archive service. Authentication is optional for metadata.
use super::*;
use std::{
    io::Read,
    sync::atomic::{AtomicBool, Ordering},
};

pub const ARCHIVES_URL: &str = "https://dev.microperceptron.com/api/v1/archives";
const MAX_PAGES: usize = 8;

#[derive(Deserialize)]
struct Response {
    archives: Vec<Archive>,
    page: Page,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    next: Option<serde::de::IgnoredAny>,
    next_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Archive {
    id: String,
    distro: String,
    version: String,
    isa: String,
    variant: String,
    file_name: String,
    size: u64,
    sha256: String,
    url: String,
    #[serde(rename = "requiresAuth")]
    _requires_auth: bool,
    #[serde(default)]
    minimum: crate::hardware::Requirements,
    kore_revision: Option<String>,
    dagfs_format_version: Option<u32>,
}

fn url(value: &str) -> Result<url::Url, String> {
    let base = url::Url::parse(ARCHIVES_URL).unwrap();
    let parsed = base
        .join(value)
        .map_err(|_| "Invalid archive service URL")?;
    if value.len() > 2048
        || value.trim() != value
        || value.bytes().any(|b| b.is_ascii_control())
        || parsed.origin() != base.origin()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Archive URLs must stay on the configured HTTPS service".into());
    }
    Ok(parsed)
}

pub fn fetch(cancelled: &AtomicBool) -> Result<Catalog, String> {
    fetch_with_account(cancelled, &crate::account::Account::default())
}

pub fn fetch_with_account(
    cancelled: &AtomicBool,
    account: &crate::account::Account,
) -> Result<Catalog, String> {
    collect(cancelled, |address, budget| {
        let response = account.catalog_request(address)?.call()
            .map_err(|_| "Cannot load official images. Check your connection and refresh; sign in for restricted images.")?;
        if response.status() != 200 {
            return Err("Archive metadata did not return HTTP 200".into());
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(budget as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Archive metadata was interrupted")?;
        Ok(bytes)
    })
}

fn collect(
    cancelled: &AtomicBool,
    mut get: impl FnMut(&str, usize) -> Result<Vec<u8>, String>,
) -> Result<Catalog, String> {
    let mut next = ARCHIVES_URL.to_owned();
    let mut visited = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut remaining = MAX_CATALOG_BYTES;
    let mut images = Vec::new();
    for _ in 0..MAX_PAGES {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Image catalog refresh cancelled".into());
        }
        if !visited.insert(next.clone()) {
            return Err("Archive pagination contains a loop".into());
        }
        let bytes = get(&next, remaining)?;
        remaining = remaining
            .checked_sub(bytes.len())
            .ok_or("Archive metadata exceeds 256 KiB")?;
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|e| format!("Invalid archive metadata: {e}"))?;
        for archive in response.archives {
            if !valid_name(&archive.id) || !ids.insert(archive.id.clone()) || ids.len() > MAX_IMAGES
            {
                return Err("Archive IDs must be valid and unique, with at most 64 entries".into());
            }
            // The service may host other products/architectures and private builds.
            if archive.isa != "x86_64" || !archive.file_name.ends_with(".iso") {
                continue;
            }
            let profile = match archive.variant.as_str() {
                "headless" | "inference" => Profile::Inference,
                "desktop" => Profile::Desktop,
                _ => continue,
            };
            let location = url(&archive.url)?;
            if archive.file_name.starts_with('.')
                || !archive
                    .file_name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
                || location.query().is_some()
                || location.path() != format!("/dl/{}/{}", archive.id, archive.file_name)
            {
                return Err(
                    "Archive download path does not match its identity and filename".into(),
                );
            }
            images.push(Image {
                id: archive.id,
                name: format!("{} {}", archive.distro, archive.variant),
                version: archive.version,
                profile,
                minimum: archive.minimum,
                architecture: archive.isa,
                format: "iso".into(),
                url: location.into(),
                sha256: archive.sha256,
                bytes: archive.size,
                kore_revision: archive.kore_revision,
                dagfs_format_version: archive.dagfs_format_version,
            });
        }
        if let Some(value) = response.page.next_url {
            let location = url(&value)?;
            if location.path() != "/api/v1/archives" {
                return Err("Invalid archive pagination path".into());
            }
            next = location.into();
        } else {
            if response.page.next.is_some() {
                return Err("Archive pagination omitted its next URL".into());
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err("Image catalog refresh cancelled".into());
            }
            let catalog = Catalog {
                schema_version: 1,
                images,
            };
            catalog.validate(false)?;
            return Ok(catalog);
        }
    }
    Err("Archive metadata exceeds eight pages".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn page() -> Value {
        json!({"authenticated":false,"archives":[{"id":"release-1","distro":"spica","version":"0.0.0a","isa":"x86_64","variant":"headless","fileName":"spica.iso","size":168382464,"sha256":"a".repeat(64),"requiresAuth":false,"url":"https://dev.microperceptron.com/dl/release-1/spica.iso"}],"page":{"next":null,"nextUrl":null}})
    }
    fn parse(value: &Value) -> Result<Catalog, String> {
        collect(&AtomicBool::new(false), |_, _| {
            Ok(serde_json::to_vec(value).unwrap())
        })
    }
    #[test]
    fn admits_the_live_shape_without_inventing_missing_provenance() {
        let catalog = parse(&page()).unwrap();
        let image = &catalog.images()[0];
        assert_eq!(image.name, "spica headless");
        assert_eq!(image.profile, Profile::Inference);
        assert_eq!(image.minimum, Default::default());
        assert!(image.kore_revision.is_none() && image.dagfs_format_version.is_none());
        let mut protected = page();
        protected["archives"][0]["requiresAuth"] = json!(true);
        assert_eq!(parse(&protected).unwrap().images().len(), 1);
        for (field, value) in [("isa", json!("aarch64")), ("variant", json!("unknown"))] {
            let mut doc = page();
            doc["archives"][0][field] = value;
            assert!(parse(&doc).unwrap().images().is_empty());
        }
    }
    #[test]
    fn rejects_bad_identity_digest_size_origin_and_compatibility() {
        for (field, value) in [
            ("id", json!("../bad")),
            ("sha256", json!("bad")),
            ("size", json!(0)),
            ("size", json!(MAX_IMAGE_BYTES + 1)),
            (
                "url",
                json!("http://dev.microperceptron.com/dl/release-1/spica.iso"),
            ),
            (
                "url",
                json!("https://elsewhere.invalid/dl/release-1/spica.iso"),
            ),
            (
                "url",
                json!("https://dev.microperceptron.com/dl/other/spica.iso"),
            ),
            ("dagfsFormatVersion", json!(0)),
        ] {
            let mut doc = page();
            doc["archives"][0][field] = value;
            assert!(parse(&doc).is_err(), "{field}");
        }
    }
    #[test]
    fn pagination_is_bounded_same_origin_and_never_returns_a_partial_catalog() {
        let mut first = page();
        first["page"]["nextUrl"] = json!("/api/v1/archives?cursor=next");
        let mut second = page();
        second["archives"][0]["id"] = json!("release-2");
        second["archives"][0]["url"] =
            json!("https://dev.microperceptron.com/dl/release-2/spica.iso");
        let mut calls = 0;
        let catalog = collect(&AtomicBool::new(false), |address, _| {
            calls += 1;
            if calls == 2 {
                assert!(address.ends_with("?cursor=next"));
            }
            Ok(serde_json::to_vec(if calls == 1 { &first } else { &second }).unwrap())
        })
        .unwrap();
        assert_eq!(catalog.images().len(), 2);
        assert!(parse(&first).is_err()); // Duplicate IDs/loop.
        first["page"]["nextUrl"] = json!("https://elsewhere.invalid/api/v1/archives");
        assert!(parse(&first).is_err());
        assert!(
            collect(&AtomicBool::new(false), |_, budget| Ok(vec![
                b' ';
                budget + 1
            ]))
            .is_err()
        );
        assert!(
            collect(&AtomicBool::new(true), |_, _| panic!(
                "cancelled request fetched"
            ))
            .is_err()
        );
    }
}
