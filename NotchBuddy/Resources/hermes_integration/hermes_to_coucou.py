#!/usr/bin/env python3
"""Bridge: Hermes outbound webhook → Coucou nb-hook.

Hermes reads this script as an outbound webhook (stdin JSON).
It forwards the payload to Coucou's `nb-hook` executable, adding the
`coucou_agent": "hermes"` field so the event appears under a dedicated
pill.

The script works on macOS (unix socket) and on Linux where the socket
path is `$XDG_RUNTIME_DIR/coucou.sock`.
"""
import json
import os
import sys
import subprocess

def main():
    # Read full stdin
    raw = sys.stdin.read()
    if not raw:
        sys.exit(0)
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError:
        sys.exit(1)
    # Ensure coucou_agent field
    if not payload.get("coucou_agent"):
        payload["coucou_agent"] = "hermes"
    # Determine nb-hook path
    if sys.platform.startswith("darwin"):
        # macOS paths – match Coucou's docs
        # App Store vs GitHub build
        home = os.path.expanduser("~")
        # Prefer container path if exists
        appstore_path = os.path.join(home, "Library", "Containers", "fr.louisraille.Coucou", "Data", "nb-hook")
        if os.path.isfile(appstore_path):
            hook_path = appstore_path
        else:
            hook_path = os.path.join(home, "Library", "Application Support", "NotchBuddy", "nb-hook")
    else:
        # Linux – use XDG_RUNTIME_DIR or fallback to home
        runtime = os.getenv("XDG_RUNTIME_DIR") or os.path.expanduser("~/.local/share/coucou")
        hook_path = os.path.join(runtime, "nb-hook")
    if not os.path.isfile(hook_path):
        # No hook executable – just exit silently
        sys.exit(0)
    # Call nb-hook with agent name
    proc = subprocess.run([hook_path, "--agent", "hermes"], input=json.dumps(payload).encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    # Exit with same code as nb-hook (0 means success, 0 also for no‑response case)
    sys.exit(proc.returncode)

if __name__ == "__main__":
    main()
