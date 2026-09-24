#!/usr/bin/env python3
"""Emit a Cargo patch for the current native Kore owners; change no manifests."""

import argparse
import json
from pathlib import Path
import tomllib

ROOTS = {
    "kore-driver": "crates/driver",
    "kore-hostlink": "crates/hostlink",
    "kore-hostlink-shmem": "crates/hostlink-shmem",
    "kore-svc-dagfs": "services/dagfs",
    "kore-workspace-sync": "crates/workspace-sync",
}
SOURCE = "https://github.com/MicroPerceptron/kore.git"


def patches(checkout: Path) -> str:
    checkout = checkout.resolve(strict=True)
    workspace = tomllib.loads((Path(__file__).resolve().parents[1] / "Cargo.toml").read_text())
    direct = {name for name, value in workspace["workspace"]["dependencies"].items()
              if isinstance(value, dict) and value.get("git") == SOURCE}
    if direct != ROOTS.keys():
        raise ValueError(f"Kore dependency inventory changed: expected {sorted(ROOTS)}, found {sorted(direct)}")
    lines = [f"[patch.{json.dumps(SOURCE)}]"]
    for name, relative in sorted(ROOTS.items()):
        directory = (checkout / relative).resolve(strict=True)
        if not directory.is_relative_to(checkout):
            raise ValueError(f"{name}: package escapes the selected checkout")
        actual = tomllib.loads((directory / "Cargo.toml").read_text())["package"]["name"]
        if actual != name:
            raise ValueError(f"{directory}: expected {name}, found {actual}")
        lines.append(f"{name} = {{ path = {json.dumps(str(directory))} }}")
    return "\n".join(lines) + "\n"


def verify_metadata(checkout: Path, metadata: dict) -> None:
    checkout = checkout.resolve(strict=True)
    reached = {node["id"] for node in metadata["resolve"]["nodes"]}
    owners = {}
    for package in metadata["packages"]:
        name = package["name"]
        if package["id"] not in reached or not (name == "kore" or name.startswith("kore-")):
            continue
        if name == "kore":
            raise ValueError("kernel package entered the host consumer graph")
        if name in owners:
            raise ValueError(f"{name}: duplicate native owners")
        if not Path(package["manifest_path"]).resolve().is_relative_to(checkout):
            raise ValueError(f"{name}: owner is outside the selected Kore checkout")
        owners[name] = package["id"]
    missing = ROOTS.keys() - owners.keys()
    if missing:
        raise ValueError(f"missing direct Kore owners: {sorted(missing)}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    parser.add_argument("--metadata", type=Path, help="verify resolved Cargo metadata instead of printing patches")
    args = parser.parse_args()
    try:
        if args.metadata:
            verify_metadata(args.checkout, json.loads(args.metadata.read_text()))
            print("host owner graph: one current checkout, no duplicate Kore owners or kernel")
        else:
            print(patches(args.checkout), end="")
    except (OSError, KeyError, ValueError) as error:
        parser.exit(1, f"local-kore: {error}\n")
