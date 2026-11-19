#!/usr/bin/env python3
"""Check portable discovery without the Unix VM crates or private desktop dependencies."""
import argparse
import pathlib
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("clippy", "test", "inventory"))
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1]
    manifest = (root / "Cargo.toml").read_text()
    shared = manifest[manifest.index("[workspace.package]"):manifest.index('[patch.')]
    with tempfile.TemporaryDirectory(prefix="vkore-devices-") as temporary:
        work = pathlib.Path(temporary)
        shutil.copytree(root / "crates/vkore-devices", work / "crates/vkore-devices")
        (work / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\nmembers = ["crates/vkore-devices"]\n'
            + shared + '\n[workspace.lints.rust]\nunsafe_op_in_unsafe_fn = "deny"\n'
        )
        shutil.copyfile(root / "Cargo.lock", work / "Cargo.lock")
        shutil.copyfile(root / "rust-toolchain.toml", work / "rust-toolchain.toml")
        if args.command == "inventory":
            command = ["cargo", "run", "--example", "inventory"]
        else:
            command = ["cargo", args.command, "--all-targets"]
            if args.command == "clippy":
                command += ["--", "-D", "warnings"]
        subprocess.run(command, cwd=work, check=True)


if __name__ == "__main__":
    main()
