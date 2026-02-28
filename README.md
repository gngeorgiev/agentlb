# agentlb

`agentlb` runs Codex with isolated per-alias `CODEX_HOME` and a background supervisor that tracks session usage in `~/.agentlb/status.json`.

## Build

```bash
cargo build --release
```

## Install

```bash
cargo install --path .
```

## Quick Start

```bash
agentlb config init
agentlb new work
agentlb new personal
agentlb list
agentlb new
agentlb supervisor
agentlb supervisor start --background
```

## Commands

- `agentlb`
  - Round-robin across existing aliases.
- `agentlb list`
  - List known sessions with email (when available) and absolute session path.
  - Email is read from each session's auth metadata; `-` means unavailable.
- `agentlb new <alias>`
  - Create alias if missing.
  - Run login once on first creation.
  - Run command in that alias.
- `agentlb new <email>`
  - Resolve email to an existing session alias and run that session.
  - Use `agentlb list` to see available alias/email mappings.
  - If zero matches: returns a not-found error.
  - If multiple matches: returns an ambiguity error and asks for explicit alias.
- `agentlb new`
  - Pick best session using `~/.agentlb/status.json` (usage-aware selection).
  - If status is unusable after retry, creates a new alias (`auto1`, `auto2`, ...).
- `agentlb rr`
  - Force round-robin across aliases.
- `agentlb last`
  - Run the most recently selected alias.
- `agentlb supervisor`
  - Print supervisor command help.
- `agentlb supervisor start --background`
  - Print current supervisor status.
  - Start supervisor in background if needed.
- `agentlb supervisor restart`
  - Stop the running supervisor (if any) and start a new background supervisor.
- `agentlb supervisor stop`
  - Stop the running supervisor (if any).
- `agentlb config init`
  - Write default config to `~/.agentlb/config.toml`.

## Session Selection Algorithm (`agentlb new`)

`agentlb new` reads only `~/.agentlb/status.json`.

### 1) Candidate filtering

A session is eligible only if:

- `health == "healthy"`
- `lastRateLimitUpdateAt` is parseable and not stale
- staleness threshold: `now - lastRateLimitUpdateAt <= 420s`

### 2) Usage-left calculation (daily + weekly balance)

From rate limit windows:

- `remaining_primary = 100 - primary.usedPercent` (short/current window)
- `remaining_secondary = 100 - secondary.usedPercent` (weekly window)
- if both exist:
  - `usageLeftPercent = 0.60 * remaining_primary + 0.40 * remaining_secondary`
- if only one exists: use that one
- if none exist: `usageLeftPercent = 30`

This makes both daily and weekly quota pressure affect picks, instead of letting only one window dominate.

### 3) Score

For each eligible session:

- `score = usageLeftPercent`
- `score -= activeTurns * 5`
- `score -= min(restartCount * 2, 20)`
- `score -= staleness_penalty`
- `staleness_penalty = clamp((age_sec * 10 / 420), 0, 10)`

Higher score wins.

### 4) Tie-breakers

When scores tie:

1. Higher `usageLeftPercent`
2. Lower `activeTurns`
3. Older `lastSelectedAt` (LRU spread)
4. Lexicographically smaller alias

### 5) Failure fallback

If no usable candidate is found:

- retry status reads for up to `3000ms`
- if still unusable, create a new alias (`autoN`) and run there

## Supervisor Behavior

On every normal `agentlb` command invocation:

- checks `~/.agentlb/supervisor.pid`
- starts supervisor if missing/dead
- continues command flow

Supervisor responsibilities:

- maintain managed `codex app-server` per active alias
- ingest JSON-RPC usage/status events
- atomically flush `~/.agentlb/status.json`
- restart crashed app-servers with exponential backoff + jitter
- run startup + periodic probes for aliases without active managed app-server
- proactively refresh rate limits before sessions reach stale threshold

## Flags

Supported on `agentlb`, `agentlb new`, `agentlb new <alias-or-email>`, `agentlb rr`, and `agentlb last`:

- `--cmd "<command string>"` override run command for this invocation
- `--login-cmd "<command string>"` override login command (new alias only)
- `-- <args...>` pass-through args appended to run command

Examples:

```bash
agentlb --cmd "codex --model gpt-5.1-codex-mini"
agentlb new work -- --search
agentlb new gngeorgiev.it@gmail.com -- --search
agentlb new -- --help
agentlb rr
agentlb last -- --search
agentlb list
agentlb supervisor
agentlb supervisor start --background
agentlb supervisor restart
agentlb supervisor stop
```

## Email-Targeted Sessions

You can run a session by account email instead of alias:

```bash
agentlb new <email>
```

How it works:

1. `agentlb` scans existing session directories under `~/.agentlb/sessions`.
2. It reads session auth metadata and extracts email.
3. It matches your input email (case-insensitive) to existing sessions.

Outcomes:

- exactly one match: session runs
- no match: actionable error telling you to create/select alias
- multiple matches: actionable error listing matching aliases

Use `agentlb list` first when you want to discover alias/email/path mappings.

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
pick_behavior = "round_robin" # currently retained for compatibility
stale_sec = 420
busy_penalty = 5
unknown_usage_left_percent = 30
usage_primary_weight_percent = 60
usage_secondary_weight_percent = 40
restart_penalty_per_restart = 2
restart_penalty_cap = 20
staleness_penalty_max = 10
```

Notes:

- `assignment_history_window` controls retained assignment timestamps per alias.
- `pick_behavior` is retained in config, but `agentlb new` now uses status-based scoring.
- The scoring factors above control how `agentlb new` balances short-window and weekly limits and how strongly it penalizes busy/restart/stale sessions.

### Scoring Config Reference

- `stale_sec`
  - What it does: excludes sessions whose rate-limit data is older than this many seconds.
  - Increase it when: status updates are less frequent and you still want to use older data.
  - Decrease it when: you want picks to rely only on very fresh usage info.
  - Typical range: `300` to `900`.

- `busy_penalty`
  - What it does: subtracts `activeTurns * busy_penalty` from score.
  - Increase it when: you want to avoid sessions currently handling turns.
  - Decrease it when: throughput matters more than spreading active work.
  - Typical range: `2` to `10`.

- `unknown_usage_left_percent`
  - What it does: fallback usage-left value when both rate-limit windows are missing.
  - Increase it when: you want unknown sessions treated as more usable.
  - Decrease it when: you want unknown sessions deprioritized.
  - Typical range: `10` to `50`.

- `usage_primary_weight_percent`
  - What it does: weight for short/current window remaining capacity in blended usage score.
  - Increase it when: short-window (daily/current) pressure matters more.
  - Decrease it when: weekly balancing should matter more.

- `usage_secondary_weight_percent`
  - What it does: weight for weekly window remaining capacity in blended usage score.
  - Increase it when: you want stronger week-level balancing across sessions.
  - Decrease it when: short-window responsiveness is more important.

- `restart_penalty_per_restart`
  - What it does: per-restart instability penalty before capping.
  - Increase it when: unstable sessions should be avoided quickly.
  - Decrease it when: occasional restarts are acceptable.
  - Typical range: `1` to `5`.

- `restart_penalty_cap`
  - What it does: maximum total restart penalty applied.
  - Increase it when: stability should heavily influence routing.
  - Decrease it when: restart history should have limited impact.
  - Typical range: `10` to `40`.

- `staleness_penalty_max`
  - What it does: max penalty as data age approaches `stale_sec`.
  - Increase it when: near-stale data should be strongly deprioritized.
  - Decrease it when: mild staleness should be tolerated.
  - Typical range: `5` to `20`.

### Tuning Guidance

- Start with defaults unless you already see poor balancing behavior.
- For stronger daily balancing:
  - raise `usage_primary_weight_percent`
  - lower `usage_secondary_weight_percent`
- For stronger weekly balancing:
  - raise `usage_secondary_weight_percent`
  - lower `usage_primary_weight_percent`
- Keep both usage weights positive; if both are set to `0`, defaults are restored.
- If picks feel too sticky to busy sessions:
  - raise `busy_penalty`
- If picks feel too noisy due to old data:
  - lower `stale_sec` or raise `staleness_penalty_max`
- If unstable sessions keep getting selected:
  - raise `restart_penalty_per_restart` and/or `restart_penalty_cap`

## Filesystem Layout

```text
~/.agentlb/
  config.toml
  supervisor.pid
  status.json
  state/
    global.json
    sessions/
      <alias>.json
  sessions/
    <alias>/
  locks/
    state.lock
```

Permissions are private (`0700` dirs, `0600` state/lock/status/pid files).

## Exit Codes

- `0` success
- `1` runtime/config/state error
- `2` invalid CLI usage
- `3` alias not found (reserved)
- `4` no sessions for auto-pick
- `5` login command failed during alias creation

## Development

```bash
cargo test
```

## Test/Debug Environment Variables

- `AGENTLB_SUPERVISOR_DISABLED=1`: disable auto-supervisor startup (useful in tests)
- `AGENTLB_DAEMON_START_TIMEOUT_MS`: wait timeout for startup detection
- `AGENTLB_PROBE_INTERVAL_SEC`: inactive-session probe interval
- `AGENTLB_PROBE_LIFETIME_SEC`: max probe process lifetime
- `AGENTLB_STATUS_FLUSH_INTERVAL_SEC`: status flush cadence
- `AGENTLB_MAX_RESTARTS_5M`: crash-loop guard threshold
- `AGENTLB_PRE_STALE_REFRESH_SEC`: how early (in seconds before `stale_sec`) supervisor refreshes rate limits proactively
- `AGENTLB_PRE_STALE_REFRESH_COOLDOWN_SEC`: cooldown between proactive refresh attempts per session
