# UNACCEPTED optional-supervisor integration candidate.
# Native boot, containment, restart and computer-use gates remain pending.
# Committed Kore payloads are disabled; make artifacts rebuilds enabled payloads.
# The exact generated diff below is required in addition to the ISO digest.
# See docs/supervisor-acceptance.md and MicroPerceptron/kore#1053.
VKORE_EXPECTED_KORE_COMMIT := c2b45da4c4cd8ad98245e1207d069fc5d73d2628
VKORE_EXPECTED_KORE_SOURCE_COMMIT := ac349b75dc23f7d20f35c78b923c14a691133cb4
VKORE_EXPECTED_ISO_SHA256 := df4c0e29b94e7629722ec42ecf7e34989eba94f0fcadeb16ab71208aa25386cc
VKORE_EXPECTED_KORE_DIFF_SHA256 := 71f3f93b587fbe81470c84ba4c2b50d7a3405334c6064dde31ab6d880626d273
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
