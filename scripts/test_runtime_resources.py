import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('runtime_resources', Path(__file__).with_name('runtime-resources.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class RuntimeResources(unittest.TestCase):
    def test_global_profile_wins_after_migration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'projects').mkdir()
            (root / 'settings.json').write_text(json.dumps({'legacy_runtime_migrated': True, 'runtime': {'virtual_cpus': 2, 'memory_mib': 2048}}))
            (root / 'projects/a.json').write_text(json.dumps({'resources': {'virtual_cpus': 8, 'memory_mib': 8192}}))
            self.assertEqual(module.resolve(root), (2, 2048))

    def test_unanimous_legacy_limits_migrate_but_conflicts_use_application_defaults(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'projects').mkdir()
            (root / 'projects/a.json').write_text(json.dumps({'resources': {'virtual_cpus': 4, 'memory_mib': 4096}}))
            self.assertEqual(module.resolve(root), (4, 4096))
            (root / 'projects/b.json').write_text(json.dumps({'resources': {'virtual_cpus': 8, 'memory_mib': 8192}}))
            self.assertEqual(module.resolve(root), (2, 2048))


if __name__ == '__main__':
    unittest.main()
