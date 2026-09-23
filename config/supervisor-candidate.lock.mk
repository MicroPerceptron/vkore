# UNACCEPTED optional-supervisor integration candidate.
# Disabled production boot passes; enabled native/UI acceptance remains pending.
# Committed Kore payloads are disabled; make artifacts rebuilds enabled payloads.
# Require both the ISO digest and exact generated payload diff.
# See docs/supervisor-acceptance.md and MicroPerceptron/kore#1053.
VKORE_EXPECTED_KORE_COMMIT := a99e0c539a2578b6b973c003f318dfe3b855ec4a
VKORE_EXPECTED_KORE_SOURCE_COMMIT := e89901a1bf05c4f7949f085548b05c80765e1fc9
VKORE_EXPECTED_ISO_SHA256 := d3da72bf02df450ae51ee80d9a3629349e6ff8db0f9b2bc5d617529df178d3c5
VKORE_EXPECTED_KORE_DIFF_SHA256 := 9ba86caf28b901d79f6beac03097062f59f8b50bf7b0ea7098f426051d1d2af0
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
