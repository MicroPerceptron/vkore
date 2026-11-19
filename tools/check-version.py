#!/usr/bin/env python3
"""Check vKore's shared release version, changelog, lockfile, and Git boundary.

Uses only Python 3.11+'s standard library; never resolves Cargo dependencies.
Release versions are normal SemVer X.Y.Z. Prerelease distribution is not enabled.
"""
import argparse
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]
NORMAL = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", re.ASCII)
BRIDGE = "crates/iced-renderer-compat"
SECTIONS = {"Breaking", "Added", "Changed", "Fixed", "Security"}


def version(value):
    if not isinstance(value, str) or not NORMAL.fullmatch(value):
        raise ValueError(f"expected a normal SemVer X.Y.Z without leading zeroes: {value!r}")
    return tuple(map(int, value.split(".")))


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def load(root, path, ref=None):
    text = git(root, "show", f"{ref}:{path}") if ref else (root / path).read_text()
    return tomllib.loads(text)


def release_entry(text, current):
    entries = list(re.finditer(r"^## \[([^\]]+)\].*$", text, re.MULTILINE))
    if not entries or entries[0][1] != current:
        raise ValueError(f"CHANGELOG.md must start with a release entry for [{current}]")
    versions = [version(entry[1]) for entry in entries]
    if any(a <= b for a, b in zip(versions, versions[1:])):
        raise ValueError("changelog release versions must be unique and newest first")
    body = text[entries[0].end():entries[1].start() if len(entries) > 1 else len(text)]
    headings = list(re.finditer(r"^### (.+)$", body, re.MULTILINE))
    if not headings:
        raise ValueError("release entry needs Added, Changed, Fixed, Security, or Breaking notes")
    for i, heading in enumerate(headings):
        if heading[1] not in SECTIONS:
            raise ValueError(f"unknown release category: {heading[1]}")
        end = headings[i + 1].start() if i + 1 < len(headings) else len(body)
        if not re.search(r"^- \S", body[heading.end():end], re.MULTILINE):
            raise ValueError(f"{heading[1]} needs at least one changelog bullet")
    return {heading[1] for heading in headings}


def next_version(previous, categories):
    major, minor, patch = previous
    if "Breaking" in categories and major > 0:
        return major + 1, 0, 0
    if "Breaking" in categories or "Added" in categories:
        return major, minor + 1, 0
    return major, minor, patch + 1


def product_change(path):
    return path.startswith(("crates/", "assets/", "runtime/")) or path in {
        "Cargo.toml", "Cargo.lock", "tools/build-qemu-runtime.py", "rust-toolchain.toml"
    }


def check(root=ROOT, base=None, tag=None):
    manifest = load(root, "Cargo.toml")
    workspace = manifest["workspace"]
    current = workspace["package"]["version"]
    current_tuple = version(current)
    owned = set()
    for member in workspace["members"]:
        package = load(root, f"{member}/Cargo.toml")["package"]
        if member == BRIDGE:
            # This is an upstream iced patch, not a vKore release package.
            continue
        if package.get("version") != {"workspace": True}:
            raise ValueError(f"{member} must inherit version.workspace = true")
        owned.add(package["name"])
    local = {p["name"]: p["version"] for p in load(root, "Cargo.lock")["package"] if "source" not in p}
    for name in owned:
        if local.get(name) != current:
            raise ValueError(f"Cargo.lock has a stale or missing version for {name}; refresh it with Cargo")
    categories = release_entry((root / "CHANGELOG.md").read_text(), current)
    if tag is not None and tag != f"v{current}":
        raise ValueError(f"release tag must be v{current}, got {tag!r}")
    if base:
        # Resolve before using a revision in path expressions. Accept no options.
        base = git(root, "rev-parse", "--verify", "--end-of-options", f"{base}^{{commit}}")
        old = load(root, "Cargo.toml", base)["workspace"]["package"].get("version")
        if old is None:
            # One-time migration from the pre-policy 0.1.0 manifests.
            old = load(root, "crates/vkored/Cargo.toml", base)["package"]["version"]
        previous = version(old)
        paths = git(root, "diff", "--name-only", base, "HEAD").splitlines()
        if current_tuple < previous:
            raise ValueError("release version must never go backwards")
        if current_tuple == previous:
            if any(map(product_change, paths)):
                raise ValueError("product changes require a version bump and a new changelog entry")
        elif current_tuple != next_version(previous, categories):
            expected = ".".join(map(str, next_version(previous, categories)))
            raise ValueError(f"changelog categories require {old} -> {expected}, got {current}")
    print(f"vKore {current}: version, packages, lockfile and changelog agree")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", help="base commit to compare (CI uses the PR base SHA)")
    parser.add_argument("--tag", help="release tag to validate against the compiled version")
    args = parser.parse_args()
    try:
        check(base=args.base, tag=args.tag)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        print(f"version check failed: {error}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
