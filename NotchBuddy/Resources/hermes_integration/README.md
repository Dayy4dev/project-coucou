# Hermes ↔ Coucou integration

Bridge script forwards Hermes shell‑hook JSON events to Coucou's `nb-hook`.

## Usage
Add the following to `~/.hermes/config.yaml` (or your profile config):
```yaml
hooks:
  post_llm_call:
    - command: "$HOME/.hermes/agent-hooks/hermes-to-coucou.sh"
      timeout: 5
  pre_tool_call:
    - command: "$HOME/.hermes/agent-hooks/hermes-to-coucou.sh"
      timeout: 5
  on_session_end:
    - command: "$HOME/.hermes/agent-hooks/hermes-to-coucou.sh"
      timeout: 5
```
Make script executable (`chmod +x`).
Restart Hermes, then any session event appears as a pill in Coucou.
