# Development snapshot; see docs/linux-setup.md for artifact provenance.
# ISO was packaged before the source snapshot commit and retains its original
# embedded build identity. Download the exact release asset; do not rebuild it
# and assume the digest will match.
VKORE_EXPECTED_KORE_COMMIT := 3916ae8bc98bc16c685524b7bbb6d8ce65a760e2
VKORE_EXPECTED_KORE_DIFF_SHA256 := e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
VKORE_EXPECTED_ISO_SHA256 := 36f49b9f4f84ab05c22c31f3ab7b080e66682ffd842bf9200d81064a2f936e71
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
