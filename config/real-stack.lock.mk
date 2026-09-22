# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := ee357da4a0fa296482e60ccd2afa25eef184c3fb
# Source identity stays fixed when the generated payloads are committed.
VKORE_EXPECTED_KORE_SOURCE_COMMIT := 91c2b1b5cf5c4d5265d8c0226485fadcc70b0ef8
VKORE_EXPECTED_ISO_SHA256 := 7e43929c73c0740ea859325000fed4bede3c3c3d03b571ca533d1387d838238a
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
