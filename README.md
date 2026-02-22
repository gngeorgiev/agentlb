# agentlb

`agentlb` runs Codex with isolated per-alias `CODEX_HOME`.

Others will come.

Each alias has its own directory at `~/.agentlb/sessions/<alias>`, so auth/config/history stay separate.

For maximum portability share the whole dir with `syncthing` with multiple machines. Maybe within your tailscale network idk.

## Build

```bash
go build ./cmd/agentlb
```

## Install

Install directly from GitHub with Go:

```bash
go install github.com/gngeorgiev/agentlb/cmd/agentlb@latest
```

Make sure `$GOPATH/bin` (or `$HOME/go/bin`) is on your `PATH`.

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
  - Auto-pick alias and run default command.
  - Pick behavior is controlled by `sessions.pick_behavior` in config.
- `agentlb new <alias>`
  - Create alias session if missing.
  - Run login command only on first creation.
  - Run default command in that alias.
- `agentlb rr`
  - Force round-robin pick explicitly.
  - Ignores `sessions.pick_behavior`.
- `agentlb last`
  - Run using the most recently selected alias.
  - Deterministic: keeps using the same alias until another command changes the last alias.
- `agentlb config init`
  - Write `~/.agentlb/config.toml` with defaults.
  - Overwrite existing config.
  - Print config path.

## Flags

Supported on `agentlb`, `agentlb new`, `agentlb new <alias>`, `agentlb rr`, and `agentlb last`:

- `--cmd "<command string>"` override run command for this invocation.
- `--login-cmd "<command string>"` override login command for this invocation (new alias only).
- `-- <args...>` pass-through args appended to run command.

Examples:

```bash
agentlb --cmd "codex --model gpt-5.1-codex-mini"
agentlb -- --search
agentlb new work --cmd "codex" -- --help
agentlb rr
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
pick_behavior = "round_robin" # round_robin | last
```

`assignment_history_window` controls how many recent assignment timestamps are retained per alias in state.
`pick_behavior` controls how `agentlb new` (without alias) chooses a session.
`agentlb rr` always uses round-robin regardless of this value.

## Round-Robin Behavior

Aliases are sorted lexicographically and selected using persisted `round_robin_index` from `~/.agentlb/state/global.json`.

With aliases `a`, `b`, `c`, runs rotate as:

- run 1 -> `a`
- run 2 -> `b`
- run 3 -> `c`
- run 4 -> `a`

`agentlb last` always runs the most recently selected alias (from state).

## Change Pick Behavior

`agentlb` supports two pick behaviors:

- `round_robin`: next alias in rotation
- `last`: most recently selected alias

Common patterns:

```bash
# Keep rotating aliases
agentlb

# Make `agentlb new` deterministic via config
# ~/.agentlb/config.toml -> pick_behavior = "last"
agentlb new

# Override config and force round-robin for this invocation
agentlb rr

# Explicitly switch to a specific alias, then keep using it deterministically
agentlb new work
agentlb new
agentlb new
agentlb last
```

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
