# UNACCEPTED optional-supervisor integration candidate.
# Native gates have recorded passes; CI and native UI acceptance remain open.
# Committed Kore payloads are disabled; make artifacts rebuilds enabled payloads.
# Require both the ISO digest and exact generated payload diff.
# See docs/supervisor-acceptance.md and MicroPerceptron/kore#1053.
VKORE_EXPECTED_KORE_COMMIT := d65ddeb357a8fbb8004bff1c1690e682f07b811f
VKORE_EXPECTED_KORE_SOURCE_COMMIT := e89901a1bf05c4f7949f085548b05c80765e1fc9
VKORE_EXPECTED_ISO_SHA256 := b8a6f1d8609d19365bc4fd00fce457f2af150eaab3c0532504cca2aebe599df1
VKORE_EXPECTED_KORE_DIFF_SHA256 := 9ba86caf28b901d79f6beac03097062f59f8b50bf7b0ea7098f426051d1d2af0
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
