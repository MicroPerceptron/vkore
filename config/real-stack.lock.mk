# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := 74ae24c87323dd7bc7ac0a12770c25ec8c8c4852
VKORE_EXPECTED_ISO_SHA256 := bfee034fa6312b3aab799e2b79031f8fe2da57d32028603541f2ee510cb4afac
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
