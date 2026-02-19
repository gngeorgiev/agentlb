package main

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"agentlb/internal/config"
	"agentlb/internal/state"
)

func TestConfigInitWritesDefaults(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)

	code := run([]string{"config", "init"})
	if code != exitOK {
		t.Fatalf("expected exit 0, got %d", code)
	}

	cfgPath := filepath.Join(home, ".agentlb", "config.toml")
	cfg, err := config.Load(cfgPath)
	if err != nil {
		t.Fatalf("load config: %v", err)
	}
	def := config.Default()
	if cfg.Runner.DefaultCommand != def.Runner.DefaultCommand {
		t.Fatalf("default command mismatch: got %q", cfg.Runner.DefaultCommand)
	}
	if cfg.Sessions.AssignmentHistoryWindow != def.Sessions.AssignmentHistoryWindow {
		t.Fatalf("assignment window mismatch: got %d", cfg.Sessions.AssignmentHistoryWindow)
	}
	if cfg.Sessions.PickBehavior != def.Sessions.PickBehavior {
		t.Fatalf("pick behavior mismatch: got %q", cfg.Sessions.PickBehavior)
	}
}

func TestNewAliasRunsLoginAndCommandWithSessionCODEXHOME(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	fake := writeFakeCodex(t)
	loginLog := filepath.Join(home, "login.log")
	runLog := filepath.Join(home, "run.log")
	argsLog := filepath.Join(home, "args.log")
	t.Setenv("AGENTLB_RECORD_LOGIN", loginLog)
	t.Setenv("AGENTLB_RECORD_RUN", runLog)
	t.Setenv("AGENTLB_RECORD_ARGS", argsLog)

	code := run([]string{"new", "a1", "--login-cmd", fake + " login", "--cmd", fake + " run", "--", "--search"})
	if code != exitOK {
		t.Fatalf("expected exit 0, got %d", code)
	}

	expectedHome := filepath.Join(home, ".agentlb", "sessions", "a1")
	loginData, err := os.ReadFile(loginLog)
	if err != nil {
		t.Fatalf("read login log: %v", err)
	}
	runData, err := os.ReadFile(runLog)
	if err != nil {
		t.Fatalf("read run log: %v", err)
	}
	if strings.TrimSpace(string(loginData)) != expectedHome {
		t.Fatalf("login CODEX_HOME mismatch: %q", strings.TrimSpace(string(loginData)))
	}
	if strings.TrimSpace(string(runData)) != expectedHome {
		t.Fatalf("run CODEX_HOME mismatch: %q", strings.TrimSpace(string(runData)))
	}

	argsData, err := os.ReadFile(argsLog)
	if err != nil {
		t.Fatalf("read args log: %v", err)
	}
	if !strings.Contains(string(argsData), "run --search") {
		t.Fatalf("expected passthrough args in run invocation, got %q", strings.TrimSpace(string(argsData)))
	}
}

func TestAutoPickRoundRobinAcrossAliases(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	fake := writeFakeCodex(t)
	runLog := filepath.Join(home, "run.log")
	t.Setenv("AGENTLB_RECORD_RUN", runLog)

	st, err := state.NewStore()
	if err != nil {
		t.Fatalf("new store: %v", err)
	}
	if err := st.EnsureLayout(); err != nil {
		t.Fatalf("ensure layout: %v", err)
	}
	cfg := config.Default()
	cfg.Runner.DefaultCommand = fake + " run"
	cfg.Runner.LoginCommand = fake + " login"
	if err := config.WriteConfigFile(st.ConfigPath, cfg); err != nil {
		t.Fatalf("write config: %v", err)
	}

	for _, alias := range []string{"a", "b"} {
		if err := os.MkdirAll(st.SessionDir(alias), 0o700); err != nil {
			t.Fatalf("mkdir session %s: %v", alias, err)
		}
	}
	now := time.Now().UTC().Format(time.RFC3339)
	if err := st.SaveSession(state.SessionState{Alias: "a", CreatedAt: now}); err != nil {
		t.Fatalf("save a: %v", err)
	}
	if err := st.SaveSession(state.SessionState{Alias: "b", CreatedAt: now}); err != nil {
		t.Fatalf("save b: %v", err)
	}

	for i := 0; i < 2; i++ {
		code := run(nil)
		if code != exitOK {
			t.Fatalf("expected exit 0, got %d", code)
		}
	}

	runData, err := os.ReadFile(runLog)
	if err != nil {
		t.Fatalf("read run log: %v", err)
	}
	lines := strings.Split(strings.TrimSpace(string(runData)), "\n")
	if len(lines) != 2 {
		t.Fatalf("expected 2 runs, got %d lines: %q", len(lines), string(runData))
	}
	expectedA := filepath.Join(home, ".agentlb", "sessions", "a")
	expectedB := filepath.Join(home, ".agentlb", "sessions", "b")
	if strings.TrimSpace(lines[0]) != expectedA {
		t.Fatalf("expected first run to use alias a, got CODEX_HOME=%q", strings.TrimSpace(lines[0]))
	}
	if strings.TrimSpace(lines[1]) != expectedB {
		t.Fatalf("expected second run to use alias b, got CODEX_HOME=%q", strings.TrimSpace(lines[1]))
	}
}

func TestLastModeUsesMostRecentlySelectedAlias(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	fake := writeFakeCodex(t)
	runLog := filepath.Join(home, "run.log")
	t.Setenv("AGENTLB_RECORD_RUN", runLog)

	st, err := state.NewStore()
	if err != nil {
		t.Fatalf("new store: %v", err)
	}
	if err := st.EnsureLayout(); err != nil {
		t.Fatalf("ensure layout: %v", err)
	}
	cfg := config.Default()
	cfg.Runner.DefaultCommand = fake + " run"
	cfg.Runner.LoginCommand = fake + " login"
	if err := config.WriteConfigFile(st.ConfigPath, cfg); err != nil {
		t.Fatalf("write config: %v", err)
	}
	for _, alias := range []string{"a", "b"} {
		if err := os.MkdirAll(st.SessionDir(alias), 0o700); err != nil {
			t.Fatalf("mkdir session %s: %v", alias, err)
		}
	}

	if code := run(nil); code != exitOK {
		t.Fatalf("first auto-pick failed: %d", code)
	}
	if code := run(nil); code != exitOK {
		t.Fatalf("second auto-pick failed: %d", code)
	}
	if code := run([]string{"last"}); code != exitOK {
		t.Fatalf("last mode failed: %d", code)
	}

	runData, err := os.ReadFile(runLog)
	if err != nil {
		t.Fatalf("read run log: %v", err)
	}
	lines := strings.Split(strings.TrimSpace(string(runData)), "\n")
	if len(lines) != 3 {
		t.Fatalf("expected 3 runs, got %d lines: %q", len(lines), string(runData))
	}
	expectedA := filepath.Join(home, ".agentlb", "sessions", "a")
	expectedB := filepath.Join(home, ".agentlb", "sessions", "b")
	if strings.TrimSpace(lines[0]) != expectedA {
		t.Fatalf("expected first run a, got %q", strings.TrimSpace(lines[0]))
	}
	if strings.TrimSpace(lines[1]) != expectedB {
		t.Fatalf("expected second run b, got %q", strings.TrimSpace(lines[1]))
	}
	if strings.TrimSpace(lines[2]) != expectedB {
		t.Fatalf("expected last mode to reuse b, got %q", strings.TrimSpace(lines[2]))
	}
}

func TestNewWithoutAliasRespectsPickBehaviorLast(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	fake := writeFakeCodex(t)
	runLog := filepath.Join(home, "run.log")
	t.Setenv("AGENTLB_RECORD_RUN", runLog)

	st, err := state.NewStore()
	if err != nil {
		t.Fatalf("new store: %v", err)
	}
	if err := st.EnsureLayout(); err != nil {
		t.Fatalf("ensure layout: %v", err)
	}
	cfg := config.Default()
	cfg.Runner.DefaultCommand = fake + " run"
	cfg.Runner.LoginCommand = fake + " login"
	cfg.Sessions.PickBehavior = "last"
	if err := config.WriteConfigFile(st.ConfigPath, cfg); err != nil {
		t.Fatalf("write config: %v", err)
	}
	for _, alias := range []string{"a", "b"} {
		if err := os.MkdirAll(st.SessionDir(alias), 0o700); err != nil {
			t.Fatalf("mkdir session %s: %v", alias, err)
		}
	}
	if err := st.SaveGlobal(state.GlobalState{LastAlias: "b"}); err != nil {
		t.Fatalf("save global: %v", err)
	}

	if code := run([]string{"new"}); code != exitOK {
		t.Fatalf("new without alias failed: %d", code)
	}

	runData, err := os.ReadFile(runLog)
	if err != nil {
		t.Fatalf("read run log: %v", err)
	}
	expectedB := filepath.Join(home, ".agentlb", "sessions", "b")
	if strings.TrimSpace(string(runData)) != expectedB {
		t.Fatalf("expected new without alias to use last alias b, got %q", strings.TrimSpace(string(runData)))
	}
}

func TestRRCommandUsesRoundRobinRegardlessOfPickBehavior(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	fake := writeFakeCodex(t)
	runLog := filepath.Join(home, "run.log")
	t.Setenv("AGENTLB_RECORD_RUN", runLog)

	st, err := state.NewStore()
	if err != nil {
		t.Fatalf("new store: %v", err)
	}
	if err := st.EnsureLayout(); err != nil {
		t.Fatalf("ensure layout: %v", err)
	}
	cfg := config.Default()
	cfg.Runner.DefaultCommand = fake + " run"
	cfg.Runner.LoginCommand = fake + " login"
	cfg.Sessions.PickBehavior = "last"
	if err := config.WriteConfigFile(st.ConfigPath, cfg); err != nil {
		t.Fatalf("write config: %v", err)
	}
	for _, alias := range []string{"a", "b"} {
		if err := os.MkdirAll(st.SessionDir(alias), 0o700); err != nil {
			t.Fatalf("mkdir session %s: %v", alias, err)
		}
	}
	if err := st.SaveGlobal(state.GlobalState{LastAlias: "b", RoundRobinIndex: 0}); err != nil {
		t.Fatalf("save global: %v", err)
	}

	if code := run([]string{"rr"}); code != exitOK {
		t.Fatalf("rr failed: %d", code)
	}

	runData, err := os.ReadFile(runLog)
	if err != nil {
		t.Fatalf("read run log: %v", err)
	}
	expectedA := filepath.Join(home, ".agentlb", "sessions", "a")
	if strings.TrimSpace(string(runData)) != expectedA {
		t.Fatalf("expected rr to use round-robin alias a, got %q", strings.TrimSpace(string(runData)))
	}
}

func writeFakeCodex(t *testing.T) string {
	t.Helper()
	binDir := filepath.Join(t.TempDir(), "bin")
	if err := os.MkdirAll(binDir, 0o755); err != nil {
		t.Fatalf("mkdir bin: %v", err)
	}
	path := filepath.Join(binDir, "fakecodex")
	script := `#!/usr/bin/env bash
set -euo pipefail
cmd="${1:-}"
shift || true
case "$cmd" in
  login)
    if [[ -n "${AGENTLB_RECORD_LOGIN:-}" ]]; then
      printf '%s\n' "${CODEX_HOME:-}" >> "$AGENTLB_RECORD_LOGIN"
    fi
    ;;
  run)
    if [[ -n "${AGENTLB_RECORD_RUN:-}" ]]; then
      printf '%s\n' "${CODEX_HOME:-}" >> "$AGENTLB_RECORD_RUN"
    fi
    if [[ -n "${AGENTLB_RECORD_ARGS:-}" ]]; then
      printf '%s\n' "run $*" >> "$AGENTLB_RECORD_ARGS"
    fi
    ;;
  *)
    ;;
esac
`
	if err := os.WriteFile(path, []byte(script), 0o755); err != nil {
		t.Fatalf("write fake script: %v", err)
	}
	return path
}
