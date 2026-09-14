#!/usr/bin/env python3
"""Exec a process in its own POSIX session on Linux and macOS.

The launcher owns this PID and terminates its process group on exit. Exec
preserves that identity so the harness and its QEMU/llama.cpp children are
cleaned up together. No intermediate supervisor or runtime is substituted.
"""

import os
import sys


def main():
    if len(sys.argv) < 2:
        raise SystemExit("usage: launch-session.py COMMAND [ARG ...]")
    os.setsid()
    os.execvp(sys.argv[1], sys.argv[1:])


if __name__ == "__main__":
    main()
