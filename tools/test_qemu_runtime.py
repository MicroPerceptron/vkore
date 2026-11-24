"""Packaging checks that do not download or build QEMU."""
import importlib.util
import json
from pathlib import Path
import tempfile
import tarfile
import unittest

spec = importlib.util.spec_from_file_location("runtime", Path(__file__).with_name("build-qemu-runtime.py"))
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class RuntimeTests(unittest.TestCase):
    def test_corruption_and_escape_are_rejected_before_execution(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "asset").write_text("expected")
            expected = runtime.digest(root / "asset")
            (root / "asset").write_text("corrupt")
            for files in ({"asset": expected}, {"../outside": expected}, {"/outside": expected}):
                (root / "bundle.json").write_text(json.dumps({"files": files}))
                with self.assertRaises(RuntimeError):
                    runtime.verify(root)

    def test_source_extraction_excludes_only_unused_edk2_host_link(self):
        with tempfile.TemporaryDirectory() as directory:
            member = tarfile.TarInfo("qemu/roms/edk2/EmulatorPkg/Unix/Host/X11IncludeHack")
            member.type = tarfile.SYMTYPE
            member.linkname = "/usr/include/X11"
            self.assertIsNone(runtime.source_filter(member, directory))
            member.name = "qemu/unexpected"
            with self.assertRaises(tarfile.AbsoluteLinkError):
                runtime.source_filter(member, directory)

    def test_a_runtime_without_every_guest_architecture_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary, firmware = runtime.TARGETS["x86_64"]
            files = {}
            for name in [binary] + [Path("share/qemu") / f for f in firmware]:
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_text(str(name))
                files[str(name)] = runtime.digest(root / name)
            (root / "bundle.json").write_text(json.dumps({"schema": 1, "version": "test", "files": files}))
            with self.assertRaisesRegex(RuntimeError, "omits"):
                runtime.verify(root)
        self.assertEqual(set(runtime.TARGETS), {"x86_64", "aarch64"})

    def test_symlinks_are_not_assets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "asset").write_text("expected")
            (root / "link").symlink_to(root / "asset")
            (root / "bundle.json").write_text(json.dumps({"files": {"link": runtime.digest(root / "asset")}}))
            with self.assertRaises(RuntimeError):
                runtime.verify(root)


if __name__ == "__main__":
    unittest.main()
