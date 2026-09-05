"""Check actual process/session behavior without launching an inference runtime."""

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import unittest


HELPER = Path(__file__).with_name("launch-session.py")


class LaunchSessionTests(unittest.TestCase):
    def test_exec_preserves_pid_and_creates_owned_session(self):
        code = (
            "import json,os; "
            "print(json.dumps([os.getpid(), os.getpgrp(), os.getsid(0)]))"
        )
        with subprocess.Popen(
            [sys.executable, str(HELPER), sys.executable, "-c", code],
            stdout=subprocess.PIPE,
            text=True,
        ) as child:
            output, _ = child.communicate(timeout=10)
            self.assertEqual(child.returncode, 0)
            self.assertEqual(json.loads(output), [child.pid] * 3)
            self.assertNotEqual(child.pid, os.getpgrp())

    def test_exit_status_is_preserved(self):
        result = subprocess.run(
            [sys.executable, str(HELPER), sys.executable, "-c", "raise SystemExit(37)"],
            timeout=10,
        )
        self.assertEqual(result.returncode, 37)

    def test_owned_process_group_can_be_terminated(self):
        code = "import time; print('ready', flush=True); time.sleep(30)"
        with subprocess.Popen(
            [sys.executable, str(HELPER), sys.executable, "-c", code],
            stdout=subprocess.PIPE,
            text=True,
        ) as child:
            try:
                self.assertEqual(child.stdout.readline().strip(), "ready")
                os.killpg(child.pid, signal.SIGTERM)
                self.assertEqual(child.wait(timeout=10), -signal.SIGTERM)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=10)


if __name__ == "__main__":
    unittest.main()
