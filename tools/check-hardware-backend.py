#!/usr/bin/env python3
"""Check public backend crates without resolving the private Axiom UI workspace.

Cargo resolves all workspace members even with -p. These are unchanged source
copies plus the repository's shared dependencies/profiles and lockfile; there
are no replacement libraries or source edits. Full desktop gates run locally
where the pinned Axiom checkout is accessible.
"""
import argparse
import pathlib
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("clippy", "test"))
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1]
    members = ["vkore-vm", "vkore-volume", "vkored"]
    source = (root / "Cargo.toml").read_text()
    shared = source[source.index("[workspace.package]"):source.index('[patch.')]
    profiles = source[source.index("[workspace.lints.rust]"):].replace('[profile.dev.package.vkore-ui]\nopt-level = 2\n', "")
    with tempfile.TemporaryDirectory(prefix="vkore-backend-") as temporary:
        work = pathlib.Path(temporary)
        for name in members:
            shutil.copytree(root / "crates" / name, work / "crates" / name)
        (work / "Cargo.toml").write_text(
            '[workspace]\nresolver = "3"\nmembers = ['
            + ", ".join(f'"crates/{name}"' for name in members)
            + "]\n\n" + shared + profiles
        )
        shutil.copyfile(root / "Cargo.lock", work / "Cargo.lock")
        shutil.copyfile(root / "rust-toolchain.toml", work / "rust-toolchain.toml")
        command = ["cargo", args.command, "--workspace"]
        if args.command == "clippy":
            command += ["--all-targets", "--", "-D", "warnings"]
        subprocess.run(command, cwd=work, check=True)


if __name__ == "__main__":
    main()
