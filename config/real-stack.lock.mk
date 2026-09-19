# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := 5108c7732e4383109bf86adb55f199fb9447bafb
VKORE_EXPECTED_ISO_SHA256 := 6303e89f44e513b16a0b164140f464cb01f9d467e2cca10364af5e157399c2b5
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
