#!/usr/bin/env python3
"""Create machine-local configuration for the pinned development snapshot."""

import argparse
import hashlib
import json
import os
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kore-root", type=Path, required=True)
    parser.add_argument("--llama-server", type=Path, required=True)
    parser.add_argument("--model", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--state", type=Path,
                        default=Path.home() / ".local/state/vkore-real")
    parser.add_argument("--gpu-layers", type=int, default=0)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    kore, server, model, workspace, state = (
        path.expanduser().resolve() for path in
        (args.kore_root, args.llama_server, args.model, args.workspace, args.state)
    )
    # The launcher currently splits QEMU arguments on whitespace; Make paths
    # also need to remain literal. Refuse unsupported paths before writing.
    for path in (root, kore, server, model, workspace, state):
        if any(c.isspace() or c in "$#\\\"'`" for c in str(path)):
            parser.error(f"unsupported whitespace or shell/Make character in path: {path}")
    if not workspace.is_dir():
        parser.error("--workspace must be an existing, narrow project directory")
    if state == workspace or workspace in state.parents:
        parser.error("--state must be outside the workspace")
    if not server.is_file() or not os.access(server, os.X_OK):
        parser.error("--llama-server must be an executable file")
    if not model.is_file():
        parser.error("--model must be an existing GGUF file")
    iso = kore / "target/kore-x86-release.iso"
    if not iso.is_file():
        parser.error("download the pinned Kore ISO first; see docs/linux-setup.md")
    if hashlib.sha256(iso.read_bytes()).hexdigest() != (
        "36f49b9f4f84ab05c22c31f3ab7b080e66682ffd842bf9200d81064a2f936e71"
    ):
        parser.error("Kore ISO does not match the development snapshot")
    project = json.loads((root / "config/real-project.template.json").read_text())
    project_path = state / "projects" / (project["id"] + ".json")
    local = root / "Makefile.local"
    if local.exists() or project_path.exists():
        parser.error("configuration already exists; edit it manually (nothing overwritten)")
    project["workspace"].update(root=str(workspace), state=str(state / "workspaces" / project["id"]))
    project["inference"].update(kore_image=str(iso), llama_server=str(server),
                                model=str(model), extra_args=["--n-gpu-layers", str(args.gpu_layers)])
    (state / "workspaces" / project["id"]).mkdir(parents=True, exist_ok=True)
    project_path.parent.mkdir(parents=True, exist_ok=True)
    with project_path.open("x") as stream:
        stream.write(json.dumps(project, indent=2) + "\n")
    with local.open("x") as stream:
        stream.write(f"KORE_ROOT := {kore}\nVKORE_STATE := {state}\n"
                     f"VKORE_PROJECT := {project['id']}\nVKORE_LISTEN := 127.0.0.1:8787\n"
                     "VKORE_REAL_STACK_LOCK := config/linux-handoff-20260914.lock.mk\n")
    print(f"Created {local} and {project_path}")
    print("Run make dev-check, then make dev-real.")


if __name__ == "__main__":
    main()
