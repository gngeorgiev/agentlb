package session

import (
	"fmt"
	"os"
	"os/exec"
	"regexp"

	"agentlb/internal/config"
	"agentlb/internal/state"
)

func ValidateAlias(alias, pattern string) error {
	re, err := regexp.Compile(pattern)
	if err != nil {
		return fmt.Errorf("invalid alias pattern %q: %w", pattern, err)
	}
	if !re.MatchString(alias) {
		return fmt.Errorf("invalid alias %q; must match %s (example: a1)", alias, pattern)
	}
	return nil
}

func EnsureSessionDir(st *state.Store, alias string) (created bool, err error) {
	dir := st.SessionDir(alias)
	if _, err := os.Stat(dir); err == nil {
		return false, nil
	} else if !os.IsNotExist(err) {
		return false, err
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return false, err
	}
	if err := os.Chmod(dir, 0o700); err != nil {
		return false, err
	}
	return true, nil
}

func RunLogin(loginCmd string, alias string, st *state.Store) error {
	bin, args, err := config.SplitCommand(loginCmd)
	if err != nil {
		return err
	}
	cmd := exec.Command(bin, args...)
	cmd.Env = append(os.Environ(), "CODEX_HOME="+st.SessionDir(alias))
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return fmt.Errorf("login command failed: %q (exit %d)", loginCmd, ee.ExitCode())
		}
		return fmt.Errorf("login command failed: %q: %w", loginCmd, err)
	}
	return nil
}

func RunCommand(runCmd string, passthrough []string, alias string, st *state.Store) (int, error) {
	bin, args, err := config.SplitCommand(runCmd)
	if err != nil {
		return 2, err
	}
	args = append(args, passthrough...)
	cmd := exec.Command(bin, args...)
	cmd.Env = append(os.Environ(), "CODEX_HOME="+st.SessionDir(alias))
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode(), nil
		}
		return 1, err
	}
	return 0, nil
}
