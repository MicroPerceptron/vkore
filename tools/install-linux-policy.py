#!/usr/bin/env python3
"""Install narrowly scoped polkit prompt descriptions for exact vKore binaries.

Run as the desktop user. pkexec authorizes the root-owned policy installation;
subsequent machine and volume prompts use these descriptions. No auth is cached.
"""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET


def policy(executables):
    root = ET.Element("policyconfig")
    ET.SubElement(root, "vendor").text = "MicroPerceptron"
    ET.SubElement(root, "vendor_url").text = "https://github.com/MicroPerceptron/vkore"
    ET.SubElement(root, "icon_name").text = "computer"
    for executable in sorted(set(map(str, executables))):
        identity = hashlib.sha256(os.fsencode(executable)).hexdigest()[:16]
        for mode, flag, description, message in [
            ("device", "--vkore-device-helper", "Prepare vKore machine hardware", "Authentication is required to prepare hardware for a vKore machine."),
            ("memory", "--vkore-memory-helper", "Allow vKore to lock guest memory", "Authentication is required to lock memory for a vKore machine."),
            ("volume-reader", "--vkore-volume-reader", "Open a vKore disk read-only", "Authentication is required to browse the selected disk read-only in vKore."),
            ("volume-identify", "--vkore-volume-identify", "Identify vKore volumes", "Authentication is required to identify DagFS on the selected disks in vKore."),
        ]:
            action = ET.SubElement(root, "action", id=f"org.microperceptron.vkore.{identity}.{mode}")
            ET.SubElement(action, "description").text = description
            ET.SubElement(action, "message").text = message
            defaults = ET.SubElement(action, "defaults")
            for key, value in [("allow_any", "no"), ("allow_inactive", "no"), ("allow_active", "auth_admin")]:
                ET.SubElement(defaults, key).text = value
            ET.SubElement(action, "annotate", key="org.freedesktop.policykit.exec.path").text = executable
            ET.SubElement(action, "annotate", key="org.freedesktop.policykit.exec.argv1").text = flag
    ET.indent(root)
    return ET.tostring(root, encoding="utf-8", xml_declaration=True) + b"\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--executable", type=Path, action="append", required=True,
                        help="Exact vkore-ui or vkore binary; repeat to cover both")
    parser.add_argument("--output", type=Path, help="Generate a policy for packaging without installing")
    args = parser.parse_args()
    executables = [p.resolve(strict=True) for p in args.executable]
    if any(not p.is_file() or not os.access(p, os.X_OK) for p in executables):
        parser.error("Every executable must be an existing executable file")
    data = policy(executables)
    if args.output:
        args.output.write_bytes(data)
        return
    if sys.platform != "linux":
        parser.error("Installation requires Linux; use --output to generate a packaging policy")
    destination = Path("/usr/share/polkit-1/actions/org.microperceptron.vkore.policy")
    # A private directory prevents other users from replacing the installation
    # input. The root-owned destination contains descriptions, never allow rules.
    with tempfile.TemporaryDirectory(prefix="vkore-policy-") as temporary:
        source = Path(temporary) / destination.name
        source.write_bytes(data)
        command = ["/usr/bin/install", "-o", "root", "-g", "root", "-m", "0644", str(source), str(destination)]
        if os.geteuid() != 0:
            command = ["/usr/bin/pkexec", "--disable-internal-agent", *command]
        subprocess.run(command, check=True, timeout=120)
    print("Installed vKore authentication descriptions. Include all desired binaries when reinstalling.")


if __name__ == "__main__":
    main()
