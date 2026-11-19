#!/usr/bin/env python3
"""Build a pinned native QEMU runtime, independently of Cargo.

Requires Python >=3.12, QEMU build dependencies and patchelf (Linux) or
Xcode command-line tools (macOS). The output is a relocatable runtime directory.
No shell interpolation, root access, or system installation is used.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import selectors
import tempfile
import shutil
import subprocess
import tarfile
import urllib.request

REPO = Path(__file__).resolve().parent.parent
BINARY = Path("bin/qemu-system-x86_64")
FIRMWARE = ("edk2-x86_64-code.fd", "edk2-i386-vars.fd")


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def dependencies(path, system):
    """Return actual dynamic-library paths; refuse unresolved dependencies."""
    if system == "Linux":
        lines = run("ldd", path, capture_output=True, text=True).stdout.splitlines()
        if any("not found" in line for line in lines):
            raise RuntimeError(f"unresolved shared library: {path}")
        return [(m[1], Path(m[1])) for line in lines if (m := re.search(r"=> (/\S+)", line))]
    lines = run("otool", "-L", path, capture_output=True, text=True).stdout.splitlines()[1:]
    result = []
    for line in lines:
        name = line.strip().split(" (", 1)[0]
        install_name = name
        if name.startswith(("/usr/lib/", "/System/Library/")):
            continue
        if name.startswith("@loader_path/"):
            name = str(path.parent / name.removeprefix("@loader_path/"))
        if not name.startswith("/"):
            raise RuntimeError(f"unresolved Mach-O library {name} in {path}")
        result.append((install_name, Path(name)))
    return result


def bundle_libraries(root, system):
    """Copy non-system dependencies and rewrite their loader paths."""
    libdir = root / "lib"
    libdir.mkdir()
    pending = [(root / BINARY, root / BINARY)]
    copied = {}
    # glibc and its loader stay matched to the target distribution.
    system_elf = re.compile(r"^(ld-linux.*|lib(c|m|pthread|dl|rt|resolv|util)\.so\..*)$")
    for original, destination in pending:
        for install_name, dependency in dependencies(original, system):
            if system == "Linux" and system_elf.match(dependency.name):
                continue
            if dependency.resolve() == original.resolve():  # Mach-O library id
                continue
            name = dependency.name
            if name in copied and copied[name] != dependency.resolve():
                raise RuntimeError(f"conflicting libraries named {name}")
            if name not in copied:
                copied[name] = dependency.resolve()
                shutil.copy2(dependency.resolve(), libdir / name)
                pending.append((dependency.resolve(), libdir / name))
            if system == "Darwin":
                # otool prints the original install name, which may differ from
                # the resolved source path only for loader-relative libraries.
                old = install_name
                run("install_name_tool", "-change", old, f"@rpath/{name}", destination)
        if system == "Linux":
            run("patchelf", "--force-rpath", "--set-rpath",
                "$ORIGIN/../lib" if destination == root / BINARY else "$ORIGIN", destination)
        else:
            if destination != root / BINARY:
                run("install_name_tool", "-id", f"@rpath/{destination.name}", destination)
            run("install_name_tool", "-add_rpath",
                "@loader_path/../lib" if destination == root / BINARY else "@loader_path", destination)
    if system == "Darwin":
        for _, path in reversed(pending):
            signing = ["codesign", "--force", "--sign", "-"]
            if path == root / BINARY:
                signing += ["--entitlements", root / "share/qemu/vkore-hvf.entitlements"]
            run(*signing, path)


def package(stage, source, output, lock, system):
    # Firmware, VGA BIOS and other data are installed by QEMU's own Meson rules.
    # Preserve the upstream license texts alongside the exact source provenance.
    licenses = stage / "licenses"
    licenses.mkdir()
    for name in ("COPYING", "COPYING.LIB"):
        shutil.copy2(source / name, licenses / name)
    shutil.copytree(source / "pc-bios" / "descriptors", licenses / "firmware-descriptors")
    for path in (source / "pc-bios").glob("*LICENSE*"):
        shutil.copy2(path, licenses / path.name)
    shutil.copy2(REPO / "packaging/qemu.lock.json", stage / "source.json")
    if system == "Darwin":
        shutil.copy2(source / "accel/hvf/entitlements.plist", stage / "share/qemu/vkore-hvf.entitlements")
    bundle_libraries(stage, system)
    # Store plain files only, including any installed symlink aliases.
    for path in sorted(stage.rglob("*")):
        if path.is_symlink():
            data = path.read_bytes()
            path.unlink()
            path.write_bytes(data)
    manifest = {"schema": 1, "version": lock["version"], "source_sha256": lock["sha256"],
                "host_os": "macos" if system == "Darwin" else "linux",
                "host_arch": {"arm64": "aarch64", "AMD64": "x86_64"}.get(platform.machine(), platform.machine()),
                "files": {str(p.relative_to(stage)): digest(p) for p in sorted(stage.rglob("*")) if p.is_file()}}
    (stage / "bundle.json").write_text(json.dumps(manifest, indent=2) + "\n")
    verify(stage)
    output.parent.mkdir(parents=True, exist_ok=True)
    stage.rename(output)
    verify(output)  # Exercise relocation, not only the build-tree installation.
    smoke(output)
    print(f"Verified QEMU runtime: {output}")


def verify(root):
    manifest = json.loads((root / "bundle.json").read_text())
    for name, expected in manifest["files"].items():
        path = Path(name)
        if path.is_absolute() or ".." in path.parts:
            raise RuntimeError("unsafe manifest path")
        full = root / path
        if full.is_symlink() or not full.is_file() or digest(full) != expected:
            raise RuntimeError(f"runtime integrity check failed: {name}")
    required = {str(BINARY)} | {f"share/qemu/{name}" for name in FIRMWARE}
    if not required.issubset(manifest["files"]):
        raise RuntimeError("runtime manifest omits executable or firmware")
    if manifest.get("schema") != 1 or not manifest.get("version"):
        raise RuntimeError("unsupported runtime manifest")
    output = run(root / BINARY, "--version", capture_output=True, text=True).stdout
    if f'version {manifest["version"]}' not in output:
        raise RuntimeError("runtime version differs from lock")
    for name in FIRMWARE:
        if not (root / "share/qemu" / name).is_file():
            raise RuntimeError(f"firmware missing: {name}")


def smoke(root):
    """Negotiate QMP against the relocated binary and bundled UEFI assets."""
    with tempfile.TemporaryDirectory() as directory:
        vars_path = Path(directory) / "vars.fd"
        shutil.copy2(root / "share/qemu" / FIRMWARE[1], vars_path)
        command = [str(root / BINARY), "-L", str(root / "share/qemu"),
                   "-machine", "q35", "-accel", "tcg", "-S", "-nodefaults",
                   "-display", "none", "-monitor", "none", "-qmp", "stdio",
                   "-drive", f"if=pflash,format=raw,readonly=on,file={root / 'share/qemu' / FIRMWARE[0]}",
                   "-drive", f"if=pflash,format=raw,file={vars_path}"]
        with tempfile.TemporaryFile() as errors:
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors, bufsize=0)
            try:
                selector = selectors.DefaultSelector()
                with selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    def message():
                        if not selector.select(15):
                            raise RuntimeError("QEMU QMP smoke test timed out")
                        line = process.stdout.readline(1 << 20)
                        return json.loads(line)
                    if "QMP" not in message():
                        raise RuntimeError("QEMU did not greet QMP")
                    for index, name in enumerate(("qmp_capabilities", "query-status", "query-cpus-fast", "quit")):
                        process.stdin.write((json.dumps({"execute": name, "id": index}) + "\n").encode())
                        process.stdin.flush()
                        reply = message()
                        while "event" in reply:
                            reply = message()
                        if reply.get("id") != index or "return" not in reply:
                            raise RuntimeError(f"QMP rejected {name}: {reply}")
                        if name == "query-status" and reply["return"]["running"]:
                            raise RuntimeError("QEMU ran before resume")
                    process.wait(timeout=15)
                    if process.returncode:
                        raise RuntimeError("QEMU did not exit cleanly")
            finally:
                if process.poll() is None:
                    process.kill()
                process.wait()
                process.stdin.close()
                process.stdout.close()


def source_filter(member, destination):
    # EDK2's optional host emulator contains a deliberate /usr/include/X11
    # symlink. We install release firmware blobs, never build that host tool.
    if member.issym() and member.name.endswith("/roms/edk2/EmulatorPkg/Unix/Host/X11IncludeHack"):
        return None
    return tarfile.data_filter(member, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--source-archive", type=Path)
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--verify", type=Path)
    args = parser.parse_args()
    if args.verify:
        verify(args.verify.resolve())
        return
    if not args.work_dir or not args.output or args.jobs < 1:
        parser.error("--work-dir, --output and a positive --jobs are required")
    system = platform.system()
    if system not in ("Linux", "Darwin"):
        parser.error("only Linux and macOS hosts are supported")
    work, output = args.work_dir.resolve(), args.output.resolve()
    if output.exists() or work.exists():
        parser.error("work and output directories must not exist (partial builds are preserved)")
    if work == output or work in output.parents or output in work.parents:
        parser.error("work and output directories must be separate")
    if shutil.disk_usage(work.parent).free < 4 << 30:
        parser.error("QEMU packaging needs at least 4 GiB of free build space")
    lock = json.loads((REPO / "packaging/qemu.lock.json").read_text())
    work.mkdir(parents=True)
    archive = args.source_archive
    if archive is None:
        archive = work / "qemu.tar.xz"
        with urllib.request.urlopen(lock["url"], timeout=60) as response, archive.open("xb") as target:
            remaining = lock["bytes"]
            while remaining:
                chunk = response.read(min(1 << 20, remaining))
                if not chunk:
                    raise RuntimeError("QEMU source download was truncated")
                target.write(chunk)
                remaining -= len(chunk)
            if response.read(1):
                raise RuntimeError("QEMU source download exceeds pinned size")
    if archive.stat().st_size != lock["bytes"] or digest(archive) != lock["sha256"]:
        raise RuntimeError("QEMU source checksum differs from packaging/qemu.lock.json")
    with tarfile.open(archive) as source:
        source.extractall(work / "source", filter=source_filter)
    source = work / "source" / f'qemu-{lock["version"]}'
    build, stage = work / "build", work / "qemu-runtime"
    build.mkdir()
    flags = [f"--prefix={stage}", "--target-list=x86_64-softmmu",
             "--without-default-features", "--disable-download", "--disable-docs",
             "--disable-tools", "--disable-guest-agent", "--disable-modules",
             "--enable-install-blobs", "--enable-tcg", "--enable-slirp",
             "--enable-vnc", "--enable-pixman", "--disable-rust"]
    flags += ["--enable-kvm", "--enable-gtk"] if system == "Linux" else ["--enable-hvf", "--enable-cocoa"]
    build_env = os.environ.copy()
    if system == "Darwin":
        # A host may have GNU binutils ahead of Apple's Mach-O tools on PATH.
        for variable, tool in (("AR", "ar"), ("RANLIB", "ranlib")):
            build_env[variable] = run("xcrun", "--find", tool, capture_output=True, text=True).stdout.strip()
    run(source / "configure", *flags, cwd=build, env=build_env)
    run("ninja", "-C", build, f"-j{args.jobs}")
    run("ninja", "-C", build, "install")
    package(stage, source, output, lock, system)


if __name__ == "__main__":
    main()
