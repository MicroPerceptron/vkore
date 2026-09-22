# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := ac9d0dc9f4aca5213ce5c0d6c494103782508f7e
# Source identity stays fixed when the generated payloads are committed.
VKORE_EXPECTED_KORE_SOURCE_COMMIT := 68dda5a411d4d1ba0d015b5f341e2b5bfa0a5daf
VKORE_EXPECTED_ISO_SHA256 := 4fb3f53c3d511c0b14fe8570bd525e3b2c5b5aff328a4d2e7dcea819e4ed0a51
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
