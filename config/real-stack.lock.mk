# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := fd383fb8705e63400e7b78762185f3208ee355c2
VKORE_EXPECTED_ISO_SHA256 := 0595467d973e59e14104923fb4df7b57f9bbe631c046e290973c36f556bb11bf
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
