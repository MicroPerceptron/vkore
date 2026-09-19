# Reviewed provenance for the real Kore developer stack.
#
# Updating either value is a security-sensitive compatibility change. Build the
# ISO from the reviewed commit, verify the resulting image, and commit both
# updates together. The developer launcher never checks out this revision or
# changes a Kore worktree automatically.
VKORE_EXPECTED_KORE_COMMIT := c3baa37be40de2214de2f36c2a690d76788ed980
VKORE_EXPECTED_ISO_SHA256 := dc9c9982b58af5e680b03cb999c66b57b1eccd0fc11294ffef21cc49d7beb659
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
