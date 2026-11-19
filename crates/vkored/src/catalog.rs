//! The bundled official-image catalog. See docs/image-catalog.md for v1.
//! Parsing validates the complete document before any entry can be selected.
use std::collections::BTreeSet;

use serde::Deserialize;

use crate::{config::valid_name, images::MAX_IMAGE_BYTES};

const MAX_CATALOG_BYTES: usize = 256 << 10;
const MAX_IMAGES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub id: String,
    pub name: String,
    pub version: String,
    pub profile: Profile,
    architecture: String,
    format: String,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    kore_revision: String,
    dagfs_format_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Desktop,
    Inference,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    schema_version: u32,
    images: Vec<Image>,
}

impl Catalog {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_CATALOG_BYTES {
            return Err("image catalog exceeds 256 KiB".into());
        }
        let catalog: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("invalid image catalog: {error}"))?;
        if catalog.schema_version != 1 {
            return Err("unsupported image catalog version".into());
        }
        if catalog.images.len() > MAX_IMAGES {
            return Err("image catalog exceeds 64 images".into());
        }
        let mut ids = BTreeSet::new();
        for image in &catalog.images {
            if !valid_name(&image.id) || !ids.insert(&image.id) {
                return Err("image IDs must be valid and unique".into());
            }
            let printable = |text: &str| {
                !text.trim().is_empty()
                    && text.len() <= 64
                    && text.bytes().all(|c| c.is_ascii_graphic() || c == b' ')
            };
            if !printable(&image.name) || !printable(&image.version) {
                return Err("image name/version must be 1 to 64 printable ASCII characters".into());
            }
            if image.architecture != "x86_64" || image.format != "iso" {
                return Err("catalog v1 supports x86_64 ISO images only".into());
            }
            let url = url::Url::parse(&image.url).map_err(|_| "invalid image URL")?;
            if image.url.len() > 2048
                || image.url.trim() != image.url
                || image.url.bytes().any(|b| b.is_ascii_control())
                || url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(
                    "official image URLs require HTTPS without credentials or fragments".into(),
                );
            }
            let hex = |text: &str, len| {
                text.len() == len
                    && text
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            };
            if !hex(&image.sha256, 64) || !hex(&image.kore_revision, 40) {
                return Err(
                    "image digest/revision must be full lowercase hexadecimal values".into(),
                );
            }
            if image.bytes == 0 || image.bytes > MAX_IMAGE_BYTES {
                return Err("image size must be nonzero and at most 64 GiB".into());
            }
            if u64::from(image.dagfs_format_version)
                != u64::from(vkore_volume::bounds::FORMAT_VERSION)
            {
                return Err("image volume format is incompatible with this vKore release".into());
            }
        }
        Ok(catalog)
    }

    pub fn images(&self) -> &[Image] {
        &self.images
    }
    pub fn get(&self, id: &str) -> Option<&Image> {
        self.images.iter().find(|image| image.id == id)
    }
}

/// Release metadata has exactly one source: the catalog built into vKore.
pub fn official() -> Result<Catalog, String> {
    Catalog::parse(include_bytes!("../assets/official-images.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn fixture() -> Value {
        json!({"schema_version":1,"images":[{
            "id":"test-desktop-1", "name":"Kore desktop", "version":"test-1",
            "profile":"desktop", "architecture":"x86_64", "format":"iso",
            "url":"https://example.invalid/test.iso", "sha256":"a".repeat(64),
            "bytes":40960,"kore_revision":"b".repeat(40),
            "dagfs_format_version":vkore_volume::bounds::FORMAT_VERSION
        }]})
    }
    fn parse(value: &Value) -> Result<Catalog, String> {
        Catalog::parse(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn bundled_catalog_is_valid_and_has_no_placeholder_releases() {
        assert!(official().unwrap().images().is_empty());
        let catalog = parse(&fixture()).unwrap();
        assert_eq!(
            catalog.get("test-desktop-1").unwrap().profile,
            Profile::Desktop
        );
        assert!(catalog.get("missing").is_none());
    }

    #[test]
    fn catalog_rejects_ambiguous_unbounded_or_untrusted_entries() {
        for (field, value) in [
            ("id", json!("../escape")),
            ("url", json!("http://example.invalid/image.iso")),
            (
                "url",
                json!("https://user:password@example.invalid/image.iso"),
            ),
            ("url", json!("https://example.invalid/image.iso#fragment")),
            ("sha256", json!("bad")),
            ("bytes", json!(0)),
            ("bytes", json!(MAX_IMAGE_BYTES + 1)),
            ("architecture", json!("aarch64")),
            ("format", json!("raw")),
            ("kore_revision", json!("abc")),
            ("dagfs_format_version", json!(0)),
            ("name", json!("invisible\u{202e}")),
            ("profile", json!("unknown")),
            ("unexpected", json!(true)),
        ] {
            let mut document = fixture();
            document["images"][0][field] = value;
            assert!(parse(&document).is_err(), "{field}");
        }
        let mut document = fixture();
        document["schema_version"] = json!(2);
        assert!(parse(&document).is_err());
        document = fixture();
        let image = document["images"][0].clone();
        document["images"].as_array_mut().unwrap().push(image);
        assert!(parse(&document).is_err());
        assert!(Catalog::parse(&vec![b' '; MAX_CATALOG_BYTES + 1]).is_err());
        document = fixture();
        document["images"][0]
            .as_object_mut()
            .unwrap()
            .remove("sha256");
        assert!(parse(&document).is_err());
    }
}
