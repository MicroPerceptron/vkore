# Issue #24 shared-assistant artifact, stacked on PR #39 / Kore #933.
# Userland source: 5173ebce12f6b6631792e8967142b87fcf4c6789.
# Download kore-x86-vkore-headless-release.iso from vkore-issue24-20260914.
# Rebuilding requires revalidation and a new digest; do not bypass these checks.
VKORE_EXPECTED_KORE_COMMIT := 3fca0fe1f8578d59c16c465089967ecb268b7f79
VKORE_EXPECTED_KORE_DIFF_SHA256 := e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
VKORE_EXPECTED_ISO_SHA256 := 6ae17033d2fca456567c35626618fcf18d60106e401e60276863b4a2df570aa0
VKORE_KORE_ISO_RELATIVE := target/kore-x86-vkore-headless-release.iso
VKORE_KORE_BOOT_FEATURES := boot-vkore-headless
VKORE_EXPECTED_KORE_SOURCE_COMMIT := 5173ebce12f6b6631792e8967142b87fcf4c6789
