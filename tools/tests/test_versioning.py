import contextlib
import importlib.util
import io
import pathlib
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("version_policy", pathlib.Path(__file__).parents[1] / "check-version.py")
POLICY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(POLICY)


class VersionPolicy(unittest.TestCase):
    def test_normal_semver_is_numeric_and_strict(self):
        self.assertLess(POLICY.version("1.9.9"), POLICY.version("1.10.0"))
        for invalid in ["01.2.3", "1.2", "v1.2.3", "1.2.3-rc.1", "1.2.3+build", "1.2.3\n", "１.2.3"]:
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                POLICY.version(invalid)

    def test_categories_require_the_right_bump(self):
        self.assertEqual(POLICY.next_version((1, 4, 9), {"Fixed"}), (1, 4, 10))
        self.assertEqual(POLICY.next_version((1, 4, 9), {"Added", "Fixed"}), (1, 5, 0))
        self.assertEqual(POLICY.next_version((1, 4, 9), {"Breaking", "Added"}), (2, 0, 0))
        self.assertEqual(POLICY.next_version((0, 4, 9), {"Breaking"}), (0, 5, 0))

    def test_notes_are_required_and_ordered(self):
        for text in ["", "## [0.2.0]\n### Added\n", "## [0.2.0]\n### Other\n- note", "## [0.2.0]\n### Added\n- note\n## [0.2.0]\n"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                POLICY.release_entry(text, "0.2.0")

    def fixture(self, legacy=False):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = pathlib.Path(directory.name)
        (root / "crates/vkored/src").mkdir(parents=True)
        (root / "crates/vkored/src/lib.rs").write_text("// initial\n")
        self.write_release(root, "0.1.0", "Added", legacy=legacy)
        self.git(root, "init", "-q")
        self.git(root, "config", "user.name", "Version test")
        self.git(root, "config", "user.email", "test@example.invalid")
        self.commit(root)
        return root, self.git(root, "rev-parse", "HEAD")

    def git(self, root, *args):
        return subprocess.check_output(["git", "-C", str(root), *args], text=True, stderr=subprocess.PIPE).strip()

    def commit(self, root):
        self.git(root, "add", ".")
        self.git(root, "commit", "-qm", "fixture")

    def write_release(self, root, value, kind, legacy=False):
        shared = "" if legacy else f'version = "{value}"\n'
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/vkored"]\n[workspace.package]\n' + shared)
        package = f'version = "{value}"' if legacy else "version.workspace = true"
        (root / "crates/vkored/Cargo.toml").write_text('[package]\nname = "vkored"\n' + package + "\n")
        (root / "Cargo.lock").write_text(f'[[package]]\nname = "vkored"\nversion = "{value}"\n')
        (root / "CHANGELOG.md").write_text(f"## [{value}]\n### {kind}\n- A meaningful change.\n")

    def check(self, root, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()):
            POLICY.check(root, **kwargs)

    def test_migrates_legacy_version_and_checks_tag(self):
        root, base = self.fixture(legacy=True)
        self.write_release(root, "0.2.0", "Added")
        self.commit(root)
        self.check(root, base=base, tag="v0.2.0")
        with self.assertRaisesRegex(ValueError, "release tag"):
            self.check(root, tag="v0.1.0")

    def test_product_changes_cannot_reuse_a_version(self):
        root, base = self.fixture()
        (root / "crates/vkored/src/lib.rs").write_text("// changed\n")
        self.commit(root)
        with self.assertRaisesRegex(ValueError, "require a version bump"):
            self.check(root, base=base)

    def test_documentation_only_does_not_force_a_release(self):
        root, base = self.fixture()
        (root / "README.md").write_text("Documentation\n")
        self.commit(root)
        self.check(root, base=base)

    def test_incorrect_bump_and_regression_fail(self):
        for value, kind in [("0.1.1", "Added"), ("0.0.9", "Fixed"), ("1.0.0", "Fixed")]:
            root, base = self.fixture()
            self.write_release(root, value, kind)
            self.commit(root)
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.check(root, base=base)

    def test_package_and_lockfile_skew_fail(self):
        root, _ = self.fixture()
        (root / "Cargo.lock").write_text('[[package]]\nname = "vkored"\nversion = "0.0.1"\n')
        with self.assertRaisesRegex(ValueError, "stale"):
            self.check(root)
        self.write_release(root, "0.1.0", "Added", legacy=True)
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/vkored"]\n[workspace.package]\nversion = "0.1.0"\n')
        with self.assertRaisesRegex(ValueError, "inherit"):
            self.check(root)


if __name__ == "__main__":
    unittest.main()
