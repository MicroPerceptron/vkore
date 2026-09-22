# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := bce68f0cb221b9cc278fe10a345645d4f96145df
# Source identity stays fixed when the generated payloads are committed.
VKORE_EXPECTED_KORE_SOURCE_COMMIT := 79b40ede71a37984d46f0e16fb9208be1c21b116
VKORE_EXPECTED_ISO_SHA256 := ed1ec4f3ed967a170a317c58590e7b2269260db5aba436c1c91c31daf335763f
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
