#!/usr/bin/env bash
set -euo pipefail

vkore_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
kore_root=${KORE_ROOT:-"$vkore_root/../kore-722-interactive-ingress"}
make_command=${MAKE:-make}
qemu=${QEMU:-$(command -v qemu-system-x86_64 || true)}

test -f "$kore_root/Cargo.toml" || {
    echo "KORE_ROOT must name the Kore checkout with frontend ingress" >&2
    exit 2
}
test -x "$qemu" || {
    echo "QEMU executable not found: $qemu" >&2
    exit 2
}
command -v "$make_command" >/dev/null 2>&1 || {
    echo "GNU Make executable not found: $make_command" >&2
    exit 2
}
command -v qemu-img >/dev/null 2>&1 || {
    echo "qemu-img is required" >&2
    exit 2
}
command -v curl >/dev/null 2>&1 || {
    echo "curl is required" >&2
    exit 2
}

runtime=$(mktemp -d "${TMPDIR:-/tmp}/vkore-interactive-gate.XXXXXX")
artifact_dir=${VKORE_GATE_ARTIFACT_DIR:-}
gate_timeout=${VKORE_GATE_TIMEOUT_SECONDS:-600}
harness_pid=
passed=0
cleanup_ran=0
cleanup() {
    if test "$cleanup_ran" = 1; then
        return
    fi
    cleanup_ran=1
    if test -n "$harness_pid"; then
        pkill -TERM -P "$harness_pid" -x qemu-system-x86_64 2>/dev/null || true
        kill "$harness_pid" 2>/dev/null || true
        wait "$harness_pid" 2>/dev/null || true
    fi
    if test -n "$artifact_dir"; then
        mkdir -p "$artifact_dir"
        cp -a -- "$runtime"/. "$artifact_dir"/
        echo "gate artifacts copied to $artifact_dir" >&2
    fi
    if test "$passed" = 1; then
        case "$runtime" in
            "${TMPDIR:-/tmp}"/vkore-interactive-gate.*) rm -rf -- "$runtime" ;;
            *) echo "refusing to remove unexpected runtime path: $runtime" >&2 ;;
        esac
    else
        echo "gate artifacts retained at $runtime" >&2
    fi
}
trap cleanup EXIT INT TERM

workspace="$runtime/workspace"
state="$runtime/state"
workspace_state="$runtime/workspace-state"
nvme="$runtime/kore.qcow2"
usb="$runtime/usb-stick.img"
vars="$runtime/ovmf-vars.fd"
serial_log="$runtime/kore-serial.log"
debug_log="$runtime/kore-debug.log"
harness_log="$runtime/harness.log"
events_file="$runtime/events.sse"
mkdir -p "$workspace/face-0" "$state" "$workspace_state"
# Exec's first stable workspace face is rooted at the imported `face-0`
# directory. Seed the exact member beneath that authority root so the broker's
# object grant resolves the same path the prompt-scoped runner will receive.
printf 'KORE_INTERACTIVE_TOOL_OUTPUT\n' >"$workspace/face-0/X"
qemu-img create -f qcow2 "$nvme" 1G >/dev/null
qemu-img create -f raw "$usb" 16M >/dev/null

make_value() {
    local name=$1
    "$make_command" -C "$kore_root" --no-print-directory -s \
        PROFILE=intel ACCEL=tcg PAGING_PROFILE=4k48 \
        MODE=release CARGO_FLAGS=--release OUT_DIR=release \
        --eval '.PHONY: _print_vkore_runtime' \
        --eval "_print_vkore_runtime: ; \$(info \$($name))" \
        _print_vkore_runtime
}

iso_relative=$(make_value BOOT_X86_ISO)
ovmf_code=$(make_value OVMF_CODE)
ovmf_vars=$(make_value OVMF_VARS)
iso="$kore_root/$iso_relative"
test -f "$iso" || {
    echo "Kore ISO is missing: $iso" >&2
    echo "run: make -C $kore_root iso-boot-x86 MODE=release CARGO_FLAGS=--release OUT_DIR=release" >&2
    exit 2
}
test -f "$ovmf_code" && test -f "$ovmf_vars" || {
    echo "OVMF firmware is unavailable" >&2
    exit 2
}
cp -- "$ovmf_vars" "$vars"

qemu_arguments=$("$make_command" -C "$kore_root" --no-print-directory -s \
    PROFILE=intel ACCEL=tcg PAGING_PROFILE=4k48 \
    MODE=release CARGO_FLAGS=--release OUT_DIR=release \
    NVME_IMG="$nvme" USB_STICK_IMG="$usb" DEBUGCON_LOG="$debug_log" \
    QEMU_X86_HOSTLINK= \
    --eval '.PHONY: _print_vkore_runtime' \
    --eval '_print_vkore_runtime: ; $(info $(QEMU_X86_ARGS))' \
    _print_vkore_runtime)
read -r -a qemu_array <<<"$qemu_arguments"
qemu_array+=(
    -device pcie-root-port,id=rp_fb,bus=pxb1,chassis=8,slot=4
    -device bochs-display,bus=rp_fb
    -drive "if=pflash,format=raw,readonly=on,file=$ovmf_code"
    -drive "if=pflash,format=raw,file=$vars"
    -serial "file:$serial_log"
)

cargo build --manifest-path "$vkore_root/Cargo.toml" -p vkored \
    --bin vkore-harnessd --bin vkore
harness="$vkore_root/target/debug/vkore-harnessd"
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
command=(
    "$harness"
    --listen "127.0.0.1:$port"
    --state "$state"
    --qemu "$qemu"
)
for argument in "${qemu_array[@]}"; do
    command+=(--qemu-arg "$argument")
done
(cd "$kore_root" && VKORE_HOSTLINK_TRACE=1 "${command[@]}") >"$harness_log" 2>&1 &
harness_pid=$!

deadline=$((SECONDS + 30))
until curl -fsS "http://127.0.0.1:$port/api/v1/health" >/dev/null 2>&1; do
    kill -0 "$harness_pid" 2>/dev/null || {
        echo "vKore harness exited during startup" >&2
        cat "$harness_log" >&2
        exit 1
    }
    test "$SECONDS" -lt "$deadline" || {
        echo "vKore harness did not become ready" >&2
        exit 1
    }
    sleep 0.1
done

project_json=$(WORKSPACE="$workspace" WORKSPACE_STATE="$workspace_state" KORE_ISO="$iso" \
    GATE_TIMEOUT="$gate_timeout" \
    python3 -c '
import json, os
print(json.dumps({
  "schema_version": 1,
  "id": "kore-live-gate",
  "name": "Kore live gate",
  "workspace": {
    "root": os.environ["WORKSPACE"],
    "state": os.environ["WORKSPACE_STATE"],
    "access": "read_only",
    "exclusions": [],
    "max_write_bytes": 0,
    "max_write_files": 0
  },
  "inference": {
    "backend": "deterministic",
    "kore_image": os.environ["KORE_ISO"],
    "llama_server": None,
    "model": None,
    "extra_args": [],
    "context_tokens": 32768,
    "output_tokens": 512,
    "threads": 2,
    "slots": 3
  },
  "addons": [],
  "approvals": {
    "default_scope": "once",
    "confirm_external_writes": True,
    "confirm_destructive": True,
    "standing_policy_enabled": False
  },
  "retention": {
    "transcript_days": 30,
    "clean_kv_on_close": True,
    "retain_raw_model_output": False
  },
  "resources": {
    "memory_mib": 4096,
    "virtual_cpus": 4,
    "prompt_seconds": int(os.environ["GATE_TIMEOUT"]),
    "agent_limit": 4
  },
  "archived": False
}))')

api="http://127.0.0.1:$port/api/v1"
json=(-H 'Content-Type: application/json')
curl -fsS "${json[@]}" -X POST "$api/projects" --data-binary "$project_json" >/dev/null
curl -fsS "${json[@]}" -X POST "$api/sessions" --data-binary \
    '{"api_version":1,"project_id":"kore-live-gate","session_id":"gate"}' >/dev/null
curl -fsS "${json[@]}" -X POST "$api/sessions/gate/prompts" --data-binary \
    '{"api_version":1,"prompt_id":"read-x","idempotency_key":"gate-read-x","text":"read X"}' >/dev/null

deadline=$((SECONDS + gate_timeout))
while test "$SECONDS" -lt "$deadline"; do
    if curl --connect-timeout 1 --max-time 2 -fsS \
        "$api/sessions/gate/events?after=0" >"$events_file" \
        && grep -q '"kind":"prompt_terminal"' "$events_file"; then
        break
    fi
    kill -0 "$harness_pid" 2>/dev/null || {
        echo "vKore harness exited while the prompt was running" >&2
        cat "$harness_log" >&2
        exit 1
    }
    sleep 0.25
done

EVENTS_FILE="$events_file" python3 -c '
import json, os
events=[]
with open(os.environ["EVENTS_FILE"], encoding="utf-8") as stream:
    for line in stream:
        if line.startswith("data: "):
            events.append(json.loads(line[6:]))
kinds={event.get("kind") for event in events}
assistant=[event for event in events if event.get("kind") == "assistant"]
assert "validation" in kinds, kinds
assert "broker_proposal" in kinds, kinds
assert "tool_call" in kinds, kinds
assert "tool_result" in kinds, kinds
assert "capability" in kinds, kinds
assert "prompt_terminal" in kinds, kinds
assert any("KORE_INTERACTIVE_TOOL_OUTPUT" in event.get("payload",{}).get("text","") for event in assistant), assistant
'

curl -fsS -X DELETE "$api/sessions/gate" >/dev/null
passed=1
echo "kore-interactive-gate: PASS"
