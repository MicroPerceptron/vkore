"""Packaging checks that do not download or build QEMU."""
import importlib.util
import json
from pathlib import Path
import tempfile
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
