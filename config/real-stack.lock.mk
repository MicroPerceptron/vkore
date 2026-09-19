# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := d7c6b9e239053c9b41c1d2b2a4fbd358cebb0dc3
VKORE_EXPECTED_ISO_SHA256 := a56192738dfae845474a9acd4e769b8f81a78b78235039b517572623d639abd4
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
