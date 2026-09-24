# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := 6d01d91e74b70c19cde1bbfe36d203064827b6a0
# Source identity stays fixed when the generated payloads are committed.
VKORE_EXPECTED_KORE_SOURCE_COMMIT := c1a0fd568bcac9b94bd9e53759bc632c4085d195
VKORE_EXPECTED_ISO_SHA256 := 843bcf183326cb0b22feea80ba6d090b23ab781a13b70890dd34f464d3e3517f
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
