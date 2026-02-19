# agentlb

`agentlb` runs Codex with isolated per-alias `CODEX_HOME` and auto-picks aliases in round-robin order.

Each alias has its own directory at `~/.agentlb/sessions/<alias>`, so auth/config/history stay separate.

## Build

```bash
go build ./cmd/agentlb
```

## Quick Start

```bash
agentlb config init
agentlb new a1
agentlb new a2
agentlb
```

## Commands

- `agentlb`
  - Auto-pick alias via round-robin and run default command.
- `agentlb new`
  - Same as `agentlb` (round-robin auto-pick run).
- `agentlb new <alias>`
  - Create alias session if missing.
  - Run login command only on first creation.
  - Run default command in that alias.
- `agentlb last`
  - Run using the most recently selected alias.
  - Deterministic: keeps using the same alias until another command changes the last alias.
- `agentlb config init`
  - Write `~/.agentlb/config.toml` with defaults.
  - Overwrite existing config.
  - Print config path.

## Flags

Supported on `agentlb`, `agentlb new`, `agentlb new <alias>`, and `agentlb last`:

- `--cmd "<command string>"` override run command for this invocation.
- `--login-cmd "<command string>"` override login command for this invocation (new alias only).
- `-- <args...>` pass-through args appended to run command.

Examples:

```bash
agentlb --cmd "codex --model gpt-5.1-codex-mini"
agentlb -- --search
agentlb new work --cmd "codex" -- --help
agentlb last -- --search
```

## SSH Login Tip

If browser auth fails over SSH, use device auth:

```bash
agentlb new a1 --login-cmd "codex login --device-auth"
```

## Config

Path: `~/.agentlb/config.toml`

```toml
[runner]
default_command = "codex"
default_command_args = []
login_command = "codex login"

[sessions]
alias_pattern = "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$"
assignment_history_window = 30
```

`assignment_history_window` controls how many recent assignment timestamps are retained per alias in state.

## Round-Robin Behavior

Aliases are sorted lexicographically and selected using persisted `round_robin_index` from `~/.agentlb/state/global.json`.

With aliases `a`, `b`, `c`, runs rotate as:

- run 1 -> `a`
- run 2 -> `b`
- run 3 -> `c`
- run 4 -> `a`

`agentlb last` always runs the most recently selected alias (from state).

## Filesystem Layout

```text
~/.agentlb/
  config.toml
  state/
    global.json
    sessions/
      <alias>.json
  sessions/
    <alias>/
  locks/
    state.lock
```

Permissions are private (`0700` dirs, `0600` state/lock files).

## Exit Codes

- `0` success
- `1` runtime/config/state error
- `2` invalid CLI usage
- `3` alias not found (reserved)
- `4` no sessions for auto-pick
- `5` login command failed during alias creation

## Development

```bash
go test ./...
```
