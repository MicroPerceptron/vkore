#!/usr/bin/env python3
"""Use the pinned checkout timestamp/identity for xorriso packaging metadata."""

from datetime import datetime, timezone
import os
import subprocess
import sys


def main():
    root = os.environ["KORE_ROOT"]
    expected = os.environ["VKORE_EXPECTED_KORE_COMMIT"]
    actual = subprocess.check_output(
        ["git", "-C", root, "rev-parse", "HEAD"], text=True
    ).strip()
    if actual != expected:
        raise SystemExit("refusing to package an unpinned Kore revision")
    if sys.argv[1:3] != ["-as", "mkisofs"]:
        raise SystemExit("expected xorriso -as mkisofs arguments")
    epoch = int(subprocess.check_output(
        ["git", "-C", root, "show", "-s", "--format=%ct", "HEAD"], text=True
    ).strip())
    stamp = datetime.fromtimestamp(epoch, timezone.utc).strftime("%Y%m%d%H%M%S00")
    guid = f"{actual[:8]}-{actual[8:12]}-{actual[12:16]}-{actual[16:20]}-{actual[20:32]}"
    arguments = [
        "xorriso", *sys.argv[1:3],
        f"--modification-date={stamp}", "--set_all_file_dates", stamp,
        "--gpt_disk_guid", guid, *sys.argv[3:],
    ]
    os.execvp(arguments[0], arguments)


if __name__ == "__main__":
    main()
