package config

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	toml "github.com/pelletier/go-toml/v2"
)

type Runner struct {
	DefaultCommand     string   `toml:"default_command"`
	DefaultCommandArgs []string `toml:"default_command_args"`
	LoginCommand       string   `toml:"login_command"`
}

type Sessions struct {
	AliasPattern            string `toml:"alias_pattern"`
	AssignmentHistoryWindow int    `toml:"assignment_history_window"`
}

type Config struct {
	Runner   Runner   `toml:"runner"`
	Sessions Sessions `toml:"sessions"`
}

func Default() Config {
	return Config{
		Runner: Runner{
			DefaultCommand:     "codex",
			DefaultCommandArgs: []string{},
			LoginCommand:       "codex login",
		},
		Sessions: Sessions{
			AliasPattern:            "^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$",
			AssignmentHistoryWindow: 30,
		},
	}
}

func AgentLBRoot() (string, error) {
	h, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(h, ".agentlb"), nil
}

func ConfigPath() (string, error) {
	root, err := AgentLBRoot()
	if err != nil {
		return "", err
	}
	return filepath.Join(root, "config.toml"), nil
}

func Load(path string) (Config, error) {
	cfg := Default()
	b, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return cfg, nil
		}
		return Config{}, err
	}
	if err := toml.Unmarshal(b, &cfg); err != nil {
		return Config{}, fmt.Errorf("parse config %s: %w", path, err)
	}
	cfg.applyDefaultsAndValidate()
	return cfg, nil
}

func (c *Config) applyDefaultsAndValidate() {
	d := Default()
	if c.Runner.DefaultCommand == "" {
		c.Runner.DefaultCommand = d.Runner.DefaultCommand
	}
	if c.Runner.LoginCommand == "" {
		c.Runner.LoginCommand = d.Runner.LoginCommand
	}
	if c.Sessions.AliasPattern == "" {
		c.Sessions.AliasPattern = d.Sessions.AliasPattern
	}
	if c.Sessions.AssignmentHistoryWindow <= 0 {
		c.Sessions.AssignmentHistoryWindow = d.Sessions.AssignmentHistoryWindow
	}
}

func EnsureDefaultConfigFile(path string, cfg Config) error {
	if _, err := os.Stat(path); err == nil {
		return nil
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	return WriteConfigFile(path, cfg)
}

func WriteConfigFile(path string, cfg Config) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	b, err := toml.Marshal(cfg)
	if err != nil {
		return err
	}
	if err := os.WriteFile(path, b, 0o600); err != nil {
		return err
	}
	return nil
}

func SplitCommand(cmdStr string) (string, []string, error) {
	fields := strings.Fields(strings.TrimSpace(cmdStr))
	if len(fields) == 0 {
		return "", nil, fmt.Errorf("empty command")
	}
	return fields[0], fields[1:], nil
}
