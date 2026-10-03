# Hermes ↔ Coucou integration

A shell-hook bridge that shows live **Hermes** sessions in the Coucou notch as
their own pill (Windows portable build verified; macOS/Linux use the same
script — see the relay search in `hermes_to_coucou.py`).

## How it works

Hermes shell hooks (`hooks:` in `config.yaml`) pipe a JSON event to a command on
**stdin**. `hermes_to_coucou.py` translates the event into the Claude Code event
names the Coucou island understands, then forwards it to `coucou-hook --agent
hermes`, which routes it to a dedicated `hermes` pill instead of Claude Code's.

| Hermes event | Coucou event |
|---|---|
| `on_session_start` | `SessionStart` |
| `pre_llm_call` | `UserPromptSubmit` |
| `pre_tool_call` | `PreToolUse` |
| `post_tool_call` | `PostToolUse` |
| `post_llm_call` | `Stop` (→ `StopFailure` when the turn failed) |
| `on_session_end` | `SessionEnd` |
| `subagent_start` / `subagent_stop` | `SubagentStart` / `SubagentStop` |

The bridge never blocks: every path exits `0` with `{}` on stdout, the relay
gives the pipe 300 ms and exits if Coucou is closed, and no state is kept
anywhere. A paused or absent Coucou costs the agent nothing.

## Install

```bash
mkdir -p ~/.hermes/agent-hooks
cp NotchBuddy/Resources/hermes_integration/hermes_to_coucou.py ~/.hermes/agent-hooks/
chmod +x ~/.hermes/agent-hooks/hermes_to_coucou.py   # POSIX; Windows needs nothing
```

Append to `~/.hermes/config.yaml` (Windows: `%LOCALAPPDATA%\hermes\config.yaml`):

> **Windows path caveat.** Hermes expands `~` to `%USERPROFILE%`
> (`C:\Users\<you>`), but the Hermes home is `%LOCALAPPDATA%\hermes` — the two
> differ. Use the **absolute path** to the script in every `command:` or the
> hook is reported "missing or not executable" by `hermes hooks doctor`.
> Environment variables are not expanded either.

```yaml
hooks:
  on_session_start:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  pre_llm_call:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  pre_tool_call:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  post_tool_call:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  post_llm_call:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  on_session_end:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  subagent_start:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
  subagent_stop:
    - command: "~/.hermes/agent-hooks/hermes_to_coucou.py"
      timeout: 5
```

Then:

1. Approve the first-use consent prompt (or start once with
   `HERMES_ACCEPT_HOOKS=1`), which records the command in
   `~/.hermes/shell-hooks-allowlist.json`.
2. `hermes hooks doctor` — every entry should be executable and allowlisted.
3. Restart Hermes.

## Verify

```bash
hermes hooks test post_llm_call     # synthetic event through the real path
```

The island should open with a `hermes` pill. `tail %LOCALAPPDATA%\Coucou\coucou.log`
shows one `hook <Event>` line per forwarded event.
