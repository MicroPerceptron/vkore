SHELL := /usr/bin/env bash
.SHELLFLAGS := -eu -o pipefail -c

-include Makefile.local
VKORE_REAL_STACK_LOCK ?= config/real-stack.lock.mk
include $(VKORE_REAL_STACK_LOCK)

KORE_ROOT ?= $(HOME)/src/kore-issue-27
VKORE_STATE ?= $(HOME)/.local/state/vkore-issue27-test
VKORE_PROJECT ?= qwen9b-issue27
VKORE_LISTEN ?= 127.0.0.1:8787
VKORE_MAKE ?= $(shell command -v gmake 2>/dev/null || command -v make)
VKORE_XORRISO ?= python3 '$(CURDIR)/scripts/reproducible-iso.py'
VKORE_KORE_BOOT_FEATURES ?= boot
VKORE_KORE_AGENT_SUPERVISOR ?= 1
VKORE_EXPECTED_KORE_SOURCE_COMMIT ?= $(VKORE_EXPECTED_KORE_COMMIT)
PROMPT ?= hello
TIMEOUT_SECONDS ?= 900

export KORE_ROOT
export VKORE_STATE
export VKORE_PROJECT
export VKORE_LISTEN
export VKORE_MAKE
export VKORE_EXPECTED_KORE_COMMIT
export VKORE_EXPECTED_ISO_SHA256
export VKORE_EXPECTED_KORE_DIFF_SHA256
export VKORE_CARGO_CONFIG
export VKORE_KORE_BOOT_FEATURES
export VKORE_KORE_AGENT_SUPERVISOR

.DEFAULT_GOAL := help

.PHONY: help dev-check dev-real dev-fast dev-debug artifacts test

help:
	@printf '%s\n' \
	  'Real vKore developer stack' \
	  '' \
	  '  make dev-check  Verify the pinned Kore commit/ISO and live project configuration' \
	  '  make dev-real   Build release vKore and open the real Kore + llama.cpp stack' \
	  '  make dev-fast   Open it using already-built release vKore binaries' \
	  '  make dev-debug  Run one live terminal prompt and retain diagnostics' \
	  '  make artifacts  Build the pinned Kore ISO once and verify its digest' \
	  '  make test       Run the vKore workspace tests' \
	  '' \
	  'Override machine-local paths in an ignored Makefile.local; see Makefile.local.example.'

dev-check:
	./vkore-axiom --require-real --release --check \
	  --kore-root "$(KORE_ROOT)" --state "$(VKORE_STATE)" --project "$(VKORE_PROJECT)"

dev-real:
	./vkore-axiom --require-real --release \
	  --kore-root "$(KORE_ROOT)" --state "$(VKORE_STATE)" --project "$(VKORE_PROJECT)"

dev-fast:
	./vkore-axiom --require-real --release --no-build \
	  --kore-root "$(KORE_ROOT)" --state "$(VKORE_STATE)" --project "$(VKORE_PROJECT)"

dev-debug:
	./vkore-axiom --require-real --release --debug \
	  --kore-root "$(KORE_ROOT)" --state "$(VKORE_STATE)" --project "$(VKORE_PROJECT)" \
	  --prompt "$(PROMPT)" --timeout-seconds "$(TIMEOUT_SECONDS)"

artifacts:
	@test -f "$(KORE_ROOT)/Cargo.toml" || { echo 'KORE_ROOT is not a Kore checkout: $(KORE_ROOT)' >&2; exit 2; }
	@test "$$(git -C "$(KORE_ROOT)" rev-parse HEAD)" = "$(VKORE_EXPECTED_KORE_COMMIT)" || { \
	  echo 'Kore checkout is not at the reviewed commit $(VKORE_EXPECTED_KORE_COMMIT)' >&2; \
	  echo 'Create or select a separate Kore worktree at that commit; this target will not change it.' >&2; \
	  exit 2; \
	}
	$(VKORE_MAKE) -C "$(KORE_ROOT)" iso-boot-x86 \
	  PROFILE=intel ACCEL=tcg PAGING_PROFILE=4k48 \
	  BOOT_FEATURES="$(VKORE_KORE_BOOT_FEATURES)" \
	  KORE_AGENT_SUPERVISOR="$(VKORE_KORE_AGENT_SUPERVISOR)" KORE_AGENT_SUPERVISOR_PROJECT=1 \
	  KORE_GIT_HEAD="$(VKORE_EXPECTED_KORE_SOURCE_COMMIT)" KORE_BUILD_STAMP="$(VKORE_EXPECTED_KORE_SOURCE_COMMIT)" \
	  MODE=release CARGO_FLAGS=--release OUT_DIR=release XORRISO="$(VKORE_XORRISO)"
	@printf '%s  %s\n' "$(VKORE_EXPECTED_ISO_SHA256)" \
	  "$(KORE_ROOT)/$(VKORE_KORE_ISO_RELATIVE)" | sha256sum -c -

test:
	cargo $(if $(VKORE_CARGO_CONFIG),--config "$(VKORE_CARGO_CONFIG)") test --workspace
