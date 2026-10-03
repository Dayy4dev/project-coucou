#!/usr/bin/env python3
"""Hermes shell-hook bridge: translate Hermes hook events into Coucou events.

Hermes writes its own event names on stdin (``pre_tool_call``, ``post_llm_call``,
``on_session_start``, ``on_session_end``, ...). The island only understands the
Claude Code names it was built for (``SessionStart``, ``PreToolUse``, ``Stop``,
...), so a raw pass-through lands in the ``default:`` arm of its switch and is
silently dropped. This script rewrites the event name and the payload fields the
island reads, tags the payload with ``--agent hermes`` so it routes to a dedicated
``hermes`` pill, and hands it to Coucou's relay (``coucou-hook`` / ``nb-hook``).

Two rules drive the design:

* **Never block the agent.** The island may be paused, hidden, or simply not
  listening. A hook that stalls is a stalled agent, so every path exits 0 and
  prints an empty JSON object. The relay itself gives the pipe 300 ms and exits
  cleanly if Coucou is closed (see ``windows/hook/src/main.rs``).
* **Never fail the turn.** No state is kept: nothing is written to disk, no
  session file is tracked, no lock is taken. Hermes reads stdout only for the
  blocking-capable ``pre_tool_call`` event, and we always answer ``{}`` — an
  object with no ``action``/``decision`` key — which parses to "no directive",
  i.e. the tool call proceeds exactly as if the hook were not installed.

Wire:

* stdin: Hermes payload ``{hook_event_name, tool_name, tool_input, session_id,
  cwd, profile, extra}``.
* stdout: ``{}`` always. Diagnostics go to stderr only.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

# Hermes event -> Coucou event the island's switch understands.
# Coucou is a Claude Code companion: it speaks Claude Code's event names.
EVENT_MAP = {
    "on_session_start": "SessionStart",
    "pre_llm_call": "UserPromptSubmit",
    "pre_tool_call": "PreToolUse",
    "post_tool_call": "PostToolUse",
    "post_tool_call_failure": "PostToolUseFailure",
    "post_llm_call": "Stop",
    "on_session_end": "SessionEnd",
    "subagent_start": "SubagentStart",
    "subagent_stop": "SubagentStop",
}

# The island truncates every displayed string to 60 chars and the relay caps
# fields at 2 kB. Cap what we forward so a long answer never bloats the pipe.
MAX_TEXT = 600


def log(message: str) -> None:
    """Diagnostics on stderr only, so Hermes never parses junk off stdout."""
    try:
        sys.stderr.write(f"[coucou] {message}\n")
        sys.stderr.flush()
    except Exception:
        pass


def relay_candidates() -> list:
    """Paths to Coucou's relay binary, most-likely first, for this platform."""
    home = os.path.expanduser("~")
    candidates = []

    if sys.platform.startswith("win"):
        local = os.environ.get("LOCALAPPDATA") or os.path.join(home, "AppData", "Local")
        roaming = os.environ.get("APPDATA") or os.path.join(home, "AppData", "Roaming")
        # The portable build copies the relay into %LOCALAPPDATA%\Coucou\bin\
        # on launch (hooks.rs::install_relay), so this is the path that exists
        # for a portable install. The others cover a bundled / dev layout.
        candidates += [
            os.path.join(local, "Coucou", "bin", "coucou-hook.exe"),
            os.path.join(local, "Coucou", "coucou-hook.exe"),
            os.path.join(roaming, "Coucou", "bin", "coucou-hook.exe"),
        ]
    elif sys.platform == "darwin":
        # macOS app writes nb-hook into its own support / container directory.
        candidates += [
            os.path.join(home, "Library", "Application Support", "NotchBuddy", "nb-hook"),
            os.path.join(
                home, "Library", "Containers", "fr.louisraille.Coucou", "Data", "nb-hook"
            ),
        ]
    else:
        runtime = os.environ.get("XDG_RUNTIME_DIR") or os.path.join(home, ".local", "share")
        candidates += [
            os.path.join(runtime, "coucou", "nb-hook"),
            os.path.join(home, ".local", "share", "coucou", "nb-hook"),
        ]
    return candidates


def find_relay():
    for path in relay_candidates():
        if os.path.isfile(path):
            return path
    return None


def clip(value):
    text = value if isinstance(value, str) else "" if value is None else str(value)
    if len(text) > MAX_TEXT:
        text = text[:MAX_TEXT] + "\u2026"
    return text


def build_payload(hermes):
    """Translate a Hermes payload into the Coucou event shape, or None to skip."""
    hermes_event = str(hermes.get("hook_event_name") or "")
    coucou_event = EVENT_MAP.get(hermes_event)
    if not coucou_event:
        # An event Coucou has no card for (pre_api_request, on_stream_delta, ...).
        # Nothing to show; skip rather than emit a card the island ignores.
        return None

    extra = hermes.get("extra") if isinstance(hermes.get("extra"), dict) else {}

    payload = {"hook_event_name": coucou_event}

    # The relay fills cwd from the process directory when it is missing, but
    # Hermes' own cwd is the real session directory, so prefer it.
    cwd = hermes.get("cwd")
    if isinstance(cwd, str) and cwd:
        payload["cwd"] = cwd

    session_id = hermes.get("session_id")
    if isinstance(session_id, str) and session_id:
        payload["session_id"] = session_id

    # The island reads `tool_name` / `tool_input` verbatim for PreToolUse.
    if isinstance(hermes.get("tool_name"), str):
        payload["tool_name"] = hermes["tool_name"]
    if isinstance(hermes.get("tool_input"), dict):
        payload["tool_input"] = hermes["tool_input"]

    # UserPromptSubmit shows `prompt`; Stop / Notification show `message`.
    if coucou_event == "UserPromptSubmit":
        asked = extra.get("user_message") or extra.get("prompt") or extra.get("message")
        if asked:
            payload["prompt"] = clip(asked)
    else:
        said = (
            extra.get("assistant_response")
            or extra.get("message")
            or extra.get("reason")
            or extra.get("summary")
        )
        if said:
            payload["message"] = clip(said)

    # A failed turn must read as StopFailure, not Stop: Stop plays the finish
    # chime and marks the pill done, which would hide the failure.
    if coucou_event == "Stop" and (
        hermes.get("failed") or extra.get("failed") or extra.get("interrupted")
    ):
        payload["hook_event_name"] = "StopFailure"

    # Tell Coucou which surface Hermes is running on so it can adapt the
    # "Open terminal" / "Open Hermes" button on the finished card.
    # HERMES_SESSION_SOURCE is "desktop" when run from the Electron app, "cli"
    # or absent from a terminal session. HERMES_DESKTOP=1 is the legacy flag.
    source = os.environ.get("HERMES_SESSION_SOURCE", "").lower()
    if source == "desktop" or os.environ.get("HERMES_DESKTOP") == "1":
        payload["coucou_surface"] = "desktop"
    else:
        payload["coucou_surface"] = "terminal"

    return payload


def main():
    try:
        raw = sys.stdin.read()
    except Exception as exc:  # noqa: BLE001 - never let the hook raise
        log(f"stdin read failed: {exc}")
        raw = ""

    # Always answer Hermes with a valid, directive-free object.
    try:
        hermes = json.loads(raw) if raw.strip() else {}
    except json.JSONDecodeError:
        log("stdin was not JSON; nothing to forward")
        hermes = {}
    if not isinstance(hermes, dict):
        hermes = {}

    try:
        payload = build_payload(hermes)
        if payload is not None:
            relay = find_relay()
            if relay is None:
                log("Coucou relay not found; is the app running?")
            else:
                # The relay gives the pipe 300 ms and exits if Coucou is closed,
                # so this cannot stall the agent. Output is suppressed: Hermes
                # reads OUR stdout, not the relay's.
                subprocess.run(
                    [relay, "--agent", "hermes"],
                    input=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=4,
                    check=False,
                )
    except Exception as exc:  # noqa: BLE001 - a bridge must never break a turn
        log(f"forwarding failed: {exc}")

    sys.stdout.write("{}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
