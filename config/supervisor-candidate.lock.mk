# UNACCEPTED optional-supervisor integration candidate.
# Native diagnostic recovery and disabled boot pass; final integrated/UI gates pending.
# Committed Kore payloads are disabled; make artifacts rebuilds enabled payloads.
# The exact generated diff below is required in addition to the ISO digest.
# See docs/supervisor-acceptance.md and MicroPerceptron/kore#1053.
VKORE_EXPECTED_KORE_COMMIT := de6206941aad8e06616cd2b5290ebfb093118c7a
VKORE_EXPECTED_KORE_SOURCE_COMMIT := 580a7c0ce446fd24eb62c2dbb52ec00a7c8c1b9c
VKORE_EXPECTED_ISO_SHA256 := 6a98af4aa9eff4005c08af874995145b33328b7ff9e04a3d107be9169f4608d3
VKORE_EXPECTED_KORE_DIFF_SHA256 := ed75af931c78f9408b44630e84cd09e71d2dde0f339dd087b6a0b73e8383cb3b
VKORE_KORE_ISO_RELATIVE := target/kore-x86-release.iso
