# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := b80e341fa936b6fd93e93293b78afb5946039fb7
# Source identity stays fixed when the generated payloads are committed.
VKORE_EXPECTED_KORE_SOURCE_COMMIT := 3ca0edd4120903414aac7d6766bb7093a2a9a9eb
VKORE_EXPECTED_ISO_SHA256 := b1a33ecdf4f764ea2a03c822e4f36e514471b8bfca19fd815a5dfef4ae795573
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
