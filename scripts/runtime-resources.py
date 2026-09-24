#!/usr/bin/env python3
"""Resolve application VM limits using the same legacy migration as SettingsStore."""
import json
import sys
from pathlib import Path


def resolve(state):
    settings_path = state / "settings.json"
    settings = json.loads(settings_path.read_text()) if settings_path.exists() else {}
    resources = settings.get("runtime")
    if not settings.get("legacy_runtime_migrated", False):
        profiles = [json.loads(path.read_text()) for path in sorted((state / "projects").glob("*.json"))]
        profiles = [p["resources"] for p in profiles if not p.get("archived", False)]
        if profiles and all((p["virtual_cpus"], p["memory_mib"]) == (profiles[0]["virtual_cpus"], profiles[0]["memory_mib"]) for p in profiles):
            resources = profiles[0]
    resources = resources or {"virtual_cpus": 2, "memory_mib": 2048}
    cpus, memory = resources["virtual_cpus"], resources["memory_mib"]
    if type(cpus) is not int or type(memory) is not int:
        raise ValueError("Application CPU and memory limits must be integers")
    return max(1, min(64, cpus)), max(512, min(65536, memory))


if __name__ == "__main__":
    print(*resolve(Path(sys.argv[1])))
