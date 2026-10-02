#!/usr/bin/env bash
# Hermes → Coucou bridge
# Reads JSON event from stdin and forwards to Coucou's nb-hook.
# Supports macOS (Unix socket) and Windows (named pipe).

payload=$(cat)

# Determine nb-hook location.
if [[ "$OSTYPE" == "darwin"* ]]; then
  # macOS – App Store or regular build.
  hook_path="${HOME}/Library/Application Support/NotchBuddy/nb-hook"
else
  # Windows – use %APPDATA% path.
  hook_path="${APPDATA//\\/\/}/NotchBuddy/nb-hook.exe"
fi

# If nb-hook not found or not executable, exit silently.
if [[ ! -x "$hook_path" ]]; then
  exit 0
fi

# Forward payload to nb-hook. Suppress its output.
printf "%s" "$payload" | "$hook_path" --agent hermes >/dev/null 2>&1

# Hermes expects JSON on stdout; empty dict signals success.
printf "{}\n"
