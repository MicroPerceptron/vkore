import importlib.util
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

spec = importlib.util.spec_from_file_location("linux_policy", Path(__file__).resolve().parents[1] / "install-linux-policy.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class PolicyTests(unittest.TestCase):
    def test_exact_paths_modes_and_uncached_admin_authentication(self):
        paths = ["/home/user/a & b/vkore-ui", "/opt/vkore/bin/vkore"]
        root = ET.fromstring(module.policy(paths + paths))
        actions = root.findall("action")
        self.assertEqual(len(actions), 8)
        self.assertEqual(len({a.get("id") for a in actions}), 8)
        for action in actions:
            annotations = {a.get("key"): a.text for a in action.findall("annotate")}
            self.assertIn(annotations["org.freedesktop.policykit.exec.path"], paths)
            self.assertIn(annotations["org.freedesktop.policykit.exec.argv1"],
                          ["--vkore-device-helper", "--vkore-memory-helper", "--vkore-volume-reader", "--vkore-volume-identify"])
            self.assertEqual(action.findtext("defaults/allow_active"), "auth_admin")
            self.assertEqual(action.findtext("defaults/allow_inactive"), "no")
            self.assertEqual(action.findtext("defaults/allow_any"), "no")
            message = action.findtext("message")
            self.assertIn("vKore", message)
            self.assertNotIn("$(", message)
            self.assertLess(len(message), 100)
        self.assertEqual(module.policy(paths), module.policy(list(reversed(paths))))


if __name__ == "__main__":
    unittest.main()
