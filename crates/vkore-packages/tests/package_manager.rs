use ed25519_dalek::{Signer, SigningKey};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;
use vkore_packages::*;

fn platform(memory: Option<u64>) -> HostPlatform {
    HostPlatform {
        operating_system: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        cpu_features: BTreeSet::new(),
        acceleration: if cfg!(windows) {
            ["whpx".into()].into_iter().collect()
        } else {
            BTreeSet::new()
        },
        available_memory_bytes: memory,
        available_accelerator_memory_bytes: None,
    }
}

fn compatibility() -> RuntimeCompatibility {
    RuntimeCompatibility {
        hostlink: 1,
        workspace: 1,
        addon_sdk: 1,
        inference_api: 1,
    }
}

fn manifest(id: &str, version: &str, class: ArtifactClass, bytes: &[u8]) -> PackageManifest {
    PackageManifest {
        schema_version: PACKAGE_MANIFEST_VERSION,
        id: id.into(),
        version: version.into(),
        class,
        channel: ReleaseChannel::Stable,
        source_url: "https://packages.example.test/artifact".into(),
        project_url: "https://example.test/project".into(),
        publisher: "test-publisher".into(),
        license: "MPL-2.0".into(),
        digest: blake3::hash(bytes).to_hex().to_string(),
        artifact_bytes: bytes.len() as u64,
        installed_bytes: bytes.len() as u64,
        platforms: vec![PlatformConstraint {
            operating_system: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            required_cpu_features: BTreeSet::new(),
            required_acceleration: None,
        }],
        compatibility: ProtocolCompatibility {
            hostlink: Some(VersionRange {
                minimum: 1,
                maximum: 1,
            }),
            workspace: Some(VersionRange {
                minimum: 1,
                maximum: 1,
            }),
            addon_sdk: Some(VersionRange {
                minimum: 1,
                maximum: 1,
            }),
            inference_api: Some(VersionRange {
                minimum: 1,
                maximum: 1,
            }),
        },
        dependencies: BTreeMap::new(),
        conflicts: BTreeSet::new(),
        entrypoints: BTreeMap::new(),
        permissions: PermissionManifest::default(),
        model: (class == ArtifactClass::Model).then(|| ModelMetadata {
            family: "test".into(),
            quantization: "Q4_K_M".into(),
            context_length: 32_768,
            chat_template: Some("chatml".into()),
            estimated_memory_bytes: 8 * 1024 * 1024 * 1024,
            accelerator_memory_bytes: None,
            redistribution_restricted: false,
        }),
        kore_boot_manifest_digest: (class == ArtifactClass::KoreGuest).then(|| "b".repeat(64)),
        health_check: Some(HealthCheckSpec {
            kind: "process".into(),
            timeout_ms: 1_000,
        }),
    }
}

fn signed(manifest: PackageManifest, signing: &SigningKey) -> SignedPackageManifest {
    let payload = serde_json::to_vec(&manifest).unwrap();
    SignedPackageManifest {
        manifest,
        signature_hex: Some(hex::encode(signing.sign(&payload).to_bytes())),
    }
}

fn trust(signing: &SigningKey, allow_unsigned_local: bool) -> TrustPolicy {
    TrustPolicy {
        trusted_publishers: [(
            "test-publisher".into(),
            hex::encode(signing.verifying_key().to_bytes()),
        )]
        .into_iter()
        .collect(),
        allow_unsigned_local,
    }
}

fn store(root: &Path, signing: &SigningKey, limit: u64) -> PackageStore {
    PackageStore::open(
        root,
        trust(signing, true),
        platform(Some(16 * 1024 * 1024 * 1024)),
        compatibility(),
        StoreLimits {
            maximum_store_bytes: limit,
        },
    )
    .unwrap()
}

fn write_artifact(root: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = root.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[derive(Default)]
struct Workloads(usize);
impl WorkloadGuard for Workloads {
    fn active_workloads(&self, _: ArtifactClass) -> usize {
        self.0
    }
}

struct Healthy(bool);
impl HealthCheck for Healthy {
    fn check(&mut self, _: &InstalledPackage) -> Result<(), String> {
        if self.0 {
            Ok(())
        } else {
            Err("launch health check failed".into())
        }
    }
}

#[test]
fn imports_existing_kore_qemu_llama_and_gguf_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[7; 32]);
    let store = store(&temp.path().join("store"), &signing, u64::MAX);
    for (index, class) in [
        ArtifactClass::KoreGuest,
        ArtifactClass::VmRuntime,
        ArtifactClass::InferenceRuntime,
        ArtifactClass::Model,
    ]
    .into_iter()
    .enumerate()
    {
        let bytes = format!("artifact-{index}").into_bytes();
        let path = write_artifact(temp.path(), &format!("artifact-{index}"), &bytes);
        let signed = SignedPackageManifest {
            manifest: manifest(&format!("package-{index}"), "1.0.0", class, &bytes),
            signature_hex: None,
        };
        let installed = store.import_local(path, signed).unwrap();
        assert_eq!(fs::read(installed.artifact_path).unwrap(), bytes);
    }
    assert_eq!(store.manifests().unwrap().len(), 4);
}

#[test]
fn signatures_digests_protocols_platforms_and_disk_limits_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[8; 32]);
    let bytes = b"verified bytes";
    let path = write_artifact(temp.path(), "artifact", bytes);

    let strict = PackageStore::open(
        temp.path().join("strict"),
        trust(&signing, false),
        platform(None),
        compatibility(),
        StoreLimits::default(),
    )
    .unwrap();
    let unsigned = SignedPackageManifest {
        manifest: manifest("runtime", "1.0.0", ArtifactClass::VmRuntime, bytes),
        signature_hex: None,
    };
    assert!(matches!(
        strict.import_local(&path, unsigned),
        Err(PackageError::Manifest(ManifestError::SignatureRequired))
    ));

    let normal_store = store(&temp.path().join("normal"), &signing, u64::MAX);
    let mut corrupt = signed(
        manifest("runtime", "1.0.0", ArtifactClass::VmRuntime, b"different"),
        &signing,
    );
    corrupt.manifest.artifact_bytes = bytes.len() as u64;
    corrupt.manifest.installed_bytes = bytes.len() as u64;
    assert!(matches!(
        normal_store.import_local(&path, corrupt),
        Err(PackageError::Manifest(ManifestError::InvalidSignature))
    ));

    let wrong_bytes = vec![b'x'; bytes.len()];
    let wrong_digest = signed(
        manifest(
            "runtime-digest",
            "1.0.0",
            ArtifactClass::VmRuntime,
            &wrong_bytes,
        ),
        &signing,
    );
    assert!(matches!(
        normal_store.import_local(&path, wrong_digest),
        Err(PackageError::DigestMismatch)
    ));

    let mut incompatible = manifest("runtime", "1.0.0", ArtifactClass::VmRuntime, bytes);
    incompatible.compatibility.hostlink = Some(VersionRange {
        minimum: 2,
        maximum: 2,
    });
    assert!(matches!(
        normal_store.import_local(&path, signed(incompatible, &signing)),
        Err(PackageError::ProtocolIncompatible)
    ));

    let mut wrong_platform = manifest("runtime", "1.0.0", ArtifactClass::VmRuntime, bytes);
    wrong_platform.platforms[0].architecture = "not-this-architecture".into();
    assert!(matches!(
        normal_store.import_local(&path, signed(wrong_platform, &signing)),
        Err(PackageError::PlatformIncompatible)
    ));

    let tiny = store(&temp.path().join("tiny"), &signing, 2);
    assert!(matches!(
        tiny.import_local(
            &path,
            signed(
                manifest("runtime", "1.0.0", ArtifactClass::VmRuntime, bytes),
                &signing
            )
        ),
        Err(PackageError::StoreLimitExceeded)
    ));
}

#[test]
fn activation_rolls_back_on_health_failure_and_refuses_live_workloads() {
    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[9; 32]);
    let store = store(&temp.path().join("store"), &signing, u64::MAX);
    let one = b"version one";
    let two = b"version two";
    let one_path = write_artifact(temp.path(), "one", one);
    let two_path = write_artifact(temp.path(), "two", two);
    let one = store
        .import_local(
            one_path,
            signed(
                manifest("llama", "1.0.0", ArtifactClass::InferenceRuntime, one),
                &signing,
            ),
        )
        .unwrap();
    let two = store
        .import_local(
            two_path,
            signed(
                manifest("llama", "2.0.0", ArtifactClass::InferenceRuntime, two),
                &signing,
            ),
        )
        .unwrap();
    let one_ref = PackageRef::from(&one.signed_manifest.manifest);
    let two_ref = PackageRef::from(&two.signed_manifest.manifest);
    store
        .activate(&one_ref, false, 1, &Workloads(0), &mut Healthy(true))
        .unwrap();
    assert_eq!(
        store.activate(&two_ref, false, 2, &Workloads(1), &mut Healthy(true)),
        Err(PackageError::ActiveWorkloads)
    );
    assert_eq!(
        store.activate(&two_ref, false, 2, &Workloads(0), &mut Healthy(false)),
        Err(PackageError::HealthCheckFailed(
            "launch health check failed".into()
        ))
    );
    assert_eq!(
        store
            .active(ArtifactClass::InferenceRuntime)
            .unwrap()
            .unwrap()
            .current,
        one_ref
    );
    let record = store
        .activate(&two_ref, false, 3, &Workloads(0), &mut Healthy(true))
        .unwrap();
    assert_eq!(record.previous, Some(one_ref.clone()));
    let rolled_back = store
        .rollback(
            ArtifactClass::InferenceRuntime,
            4,
            &Workloads(0),
            &mut Healthy(true),
        )
        .unwrap();
    assert_eq!(rolled_back.current, one_ref);
}

#[test]
fn addon_permission_expansion_requires_separate_approval() {
    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[10; 32]);
    let store = store(&temp.path().join("store"), &signing, u64::MAX);
    let one = b"addon one";
    let two = b"addon two";
    let one_path = write_artifact(temp.path(), "one", one);
    let two_path = write_artifact(temp.path(), "two", two);
    let one = store
        .import_local(
            one_path,
            signed(
                manifest("gmail", "1.0.0", ArtifactClass::Addon, one),
                &signing,
            ),
        )
        .unwrap();
    let mut expanded = manifest("gmail", "2.0.0", ArtifactClass::Addon, two);
    expanded
        .permissions
        .network_destinations
        .insert("gmail.googleapis.com:443".into());
    expanded
        .permissions
        .tool_effects
        .insert("send_email".into());
    let two = store
        .import_local(two_path, signed(expanded, &signing))
        .unwrap();
    let one_ref = PackageRef::from(&one.signed_manifest.manifest);
    let two_ref = PackageRef::from(&two.signed_manifest.manifest);
    store
        .activate(&one_ref, false, 1, &Workloads(0), &mut Healthy(true))
        .unwrap();
    assert!(matches!(
        store.activate(&two_ref, false, 2, &Workloads(0), &mut Healthy(true)),
        Err(PackageError::PermissionExpansion(_))
    ));
    store
        .activate(&two_ref, true, 2, &Workloads(0), &mut Healthy(true))
        .unwrap();
}

#[test]
fn model_fit_and_release_selection_are_explicit() {
    let model = manifest("model", "1.0.0", ArtifactClass::Model, b"model")
        .model
        .unwrap();
    assert_eq!(
        model_fit(&model, &platform(Some(4 * 1024 * 1024 * 1024))),
        ModelFit::DoesNotFit
    );
    assert_eq!(
        model_fit(&model, &platform(Some(9 * 1024 * 1024 * 1024))),
        ModelFit::Tight
    );
    assert_eq!(
        model_fit(&model, &platform(Some(16 * 1024 * 1024 * 1024))),
        ModelFit::Fits
    );
    assert_eq!(model_fit(&model, &platform(None)), ModelFit::Unknown);

    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[14; 32]);
    let store = store(&temp.path().join("store"), &signing, u64::MAX);
    for version in ["1.0.0", "1.1.0"] {
        let bytes = format!("model-{version}").into_bytes();
        let path = write_artifact(temp.path(), version, &bytes);
        store
            .import_local(
                path,
                signed(
                    manifest("model-select", version, ArtifactClass::Model, &bytes),
                    &signing,
                ),
            )
            .unwrap();
    }
    assert_eq!(
        store
            .select_release("model-select", ReleaseChannel::Stable, None)
            .unwrap()
            .version,
        "1.1.0"
    );
    assert_eq!(
        store
            .select_release("model-select", ReleaseChannel::Stable, Some("1.0.0"))
            .unwrap()
            .version,
        "1.0.0"
    );
}

#[test]
fn offline_bundle_round_trips_and_cleanup_is_explicit() {
    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[11; 32]);
    let source = store(&temp.path().join("source"), &signing, u64::MAX);
    let bytes = b"offline package";
    let path = write_artifact(temp.path(), "artifact", bytes);
    let installed = source
        .import_local(
            path,
            signed(
                manifest("qemu", "1.0.0", ArtifactClass::VmRuntime, bytes),
                &signing,
            ),
        )
        .unwrap();
    let reference = PackageRef::from(&installed.signed_manifest.manifest);
    let bundle = temp.path().join("bundle.tar");
    source.export_bundle(&reference, &bundle).unwrap();

    let destination = PackageStore::open(
        temp.path().join("destination"),
        trust(&signing, false),
        platform(None),
        compatibility(),
        StoreLimits::default(),
    )
    .unwrap();
    let imported = destination.import_bundle(bundle).unwrap();
    let imported_ref = PackageRef::from(&imported.signed_manifest.manifest);
    destination.unregister(&imported_ref).unwrap();
    let candidates = destination.cleanup_candidates().unwrap();
    assert_eq!(
        candidates,
        [imported_ref.digest.clone()].into_iter().collect()
    );
    assert_eq!(
        destination.remove_unreferenced(&candidates).unwrap(),
        bytes.len() as u64
    );
}

#[test]
fn pinned_http_download_resumes_and_verifies_before_install() {
    let temp = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[12; 32]);
    let store = PackageStore::open(
        temp.path().join("store"),
        trust(&signing, false),
        platform(None),
        compatibility(),
        StoreLimits::default(),
    )
    .unwrap();
    let bytes = b"resumable signed package".to_vec();
    let manifest = manifest("downloaded", "1.0.0", ArtifactClass::VmRuntime, &bytes);
    let signed = signed(manifest, &signing);
    let partial = store.partial_path(&signed.manifest.digest);
    fs::write(&partial, &bytes[..7]).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server_bytes = bytes.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 4096];
        let read = stream.read(&mut request).unwrap();
        let request = String::from_utf8_lossy(&request[..read]);
        assert!(request.to_ascii_lowercase().contains("range: bytes=7-"));
        let body = &server_bytes[7..];
        write!(
            stream,
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    let installed = store
        .download_and_install(&format!("http://{address}/artifact"), signed)
        .unwrap();
    server.join().unwrap();
    assert_eq!(fs::read(installed.artifact_path).unwrap(), bytes);
    assert!(!partial.exists());
}
