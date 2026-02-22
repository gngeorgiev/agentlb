package main

import (
	"fmt"
	"os"
	"strings"

	"agentlb/internal/config"
	"agentlb/internal/session"
	"agentlb/internal/state"
)

const (
	exitOK            = 0
	exitGeneric       = 1
	exitUsage         = 2
	exitAliasNotFound = 3
	exitNoSessions    = 4
	exitLoginFailed   = 5
)

type cliArgs struct {
	mode        string
	alias       string
	cmd         string
	loginCmd    string
	passthrough []string
}

func main() {
	os.Exit(run(os.Args[1:]))
}

func run(argv []string) int {
	a, err := parseCLI(argv)
	if err != nil {
		eprintln(err.Error())
		return exitUsage
	}

	st, err := state.NewStore()
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}
	if err := st.EnsureLayout(); err != nil {
		eprintln(err.Error())
		return exitGeneric
	}

	if a.mode == "config_init" {
		if err := config.WriteConfigFile(st.ConfigPath, config.Default()); err != nil {
			eprintln(err.Error())
			return exitGeneric
		}
		fmt.Fprintln(os.Stdout, st.ConfigPath)
		return exitOK
	}

	cfgPath := st.ConfigPath
	cfg, err := config.Load(cfgPath)
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}

	runCmd := resolveRunCommand(a, cfg)
	loginCmd := resolveLoginCommand(a, cfg, runCmd)

	switch {
	case a.mode == "new" && a.alias != "":
		return runNewAlias(a.alias, runCmd, loginCmd, a.passthrough, cfg, st)
	case a.mode == "new" && a.alias == "":
		return runAutoPick(runCmd, a.passthrough, cfg, st, cfg.Sessions.PickBehavior)
	case a.mode == "rr":
		return runAutoPick(runCmd, a.passthrough, cfg, st, "round_robin")
	case a.mode == "last":
		return runLast(runCmd, a.passthrough, cfg, st)
	case a.mode == "root":
		return runAutoPick(runCmd, a.passthrough, cfg, st, "round_robin")
	default:
		eprintln("invalid mode")
		return exitUsage
	}
}

func runNewAlias(alias, runCmd, loginCmd string, passthrough []string, cfg config.Config, st *state.Store) int {
	if err := session.ValidateAlias(alias, cfg.Sessions.AliasPattern); err != nil {
		eprintln(err.Error())
		return exitUsage
	}

	unlock, err := st.Lock()
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}
	created, err := session.EnsureSessionDir(st, alias)
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if err := config.EnsureDefaultConfigFile(st.ConfigPath, cfg); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if err := st.RecordAssignment(alias, cfg.Sessions.AssignmentHistoryWindow); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	g, err := st.LoadGlobal()
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	g.LastAlias = alias
	if err := st.SaveGlobal(g); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	unlock()

	if created {
		if err := session.RunLogin(loginCmd, alias, st); err != nil {
			eprintln(err.Error())
			return exitLoginFailed
		}
	}

	code, err := session.RunCommand(runCmd, passthrough, alias, st)
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}
	return code
}

func runAutoPick(runCmd string, passthrough []string, cfg config.Config, st *state.Store, pickBehavior string) int {
	unlock, err := st.Lock()
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}

	aliases, err := st.ListAliases()
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if len(aliases) == 0 {
		unlock()
		eprintln("no managed sessions found; create one with: agentlb new <alias>")
		return exitNoSessions
	}

	g, err := st.LoadGlobal()
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	winner, err := pickAlias(aliases, &g, pickBehavior)
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitNoSessions
	}
	g.LastAlias = winner
	if err := config.EnsureDefaultConfigFile(st.ConfigPath, cfg); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if err := st.RecordAssignment(winner, cfg.Sessions.AssignmentHistoryWindow); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if err := st.SaveGlobal(g); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}

	unlock()
	code, err := session.RunCommand(runCmd, passthrough, winner, st)
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}
	return code
}

func pickAlias(aliases []string, g *state.GlobalState, pickBehavior string) (string, error) {
	switch pickBehavior {
	case "last":
		if g.LastAlias == "" {
			return "", fmt.Errorf("no previous session recorded; run agentlb or agentlb new <alias> first")
		}
		for _, a := range aliases {
			if a == g.LastAlias {
				return g.LastAlias, nil
			}
		}
		return "", fmt.Errorf("last session no longer exists; run agentlb or agentlb new <alias> first")
	case "round_robin":
		fallthrough
	default:
		winner := aliases[g.RoundRobinIndex%len(aliases)]
		g.RoundRobinIndex = (g.RoundRobinIndex + 1) % len(aliases)
		return winner, nil
	}
}

func runLast(runCmd string, passthrough []string, cfg config.Config, st *state.Store) int {
	unlock, err := st.Lock()
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}

	aliases, err := st.ListAliases()
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if len(aliases) == 0 {
		unlock()
		eprintln("no managed sessions found; create one with: agentlb new <alias>")
		return exitNoSessions
	}

	g, err := st.LoadGlobal()
	if err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if g.LastAlias == "" {
		unlock()
		eprintln("no previous session recorded; run agentlb or agentlb new <alias> first")
		return exitNoSessions
	}
	found := false
	for _, a := range aliases {
		if a == g.LastAlias {
			found = true
			break
		}
	}
	if !found {
		unlock()
		eprintln("last session no longer exists; run agentlb or agentlb new <alias> first")
		return exitNoSessions
	}

	if err := config.EnsureDefaultConfigFile(st.ConfigPath, cfg); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if err := st.RecordAssignment(g.LastAlias, cfg.Sessions.AssignmentHistoryWindow); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}
	if err := st.SaveGlobal(g); err != nil {
		unlock()
		eprintln(err.Error())
		return exitGeneric
	}

	unlock()
	code, err := session.RunCommand(runCmd, passthrough, g.LastAlias, st)
	if err != nil {
		eprintln(err.Error())
		return exitGeneric
	}
	return code
}

func resolveRunCommand(a cliArgs, cfg config.Config) string {
	if a.cmd != "" {
		return a.cmd
	}
	parts := []string{cfg.Runner.DefaultCommand}
	parts = append(parts, cfg.Runner.DefaultCommandArgs...)
	return strings.TrimSpace(strings.Join(parts, " "))
}

func resolveLoginCommand(a cliArgs, cfg config.Config, runCmd string) string {
	if a.loginCmd != "" {
		return a.loginCmd
	}
	if strings.TrimSpace(cfg.Runner.LoginCommand) != "" {
		return cfg.Runner.LoginCommand
	}
	bin, _, err := config.SplitCommand(runCmd)
	if err != nil {
		return "codex login"
	}
	return bin + " login"
}

func parseCLI(args []string) (cliArgs, error) {
	out := cliArgs{mode: "root"}
	for i := 0; i < len(args); i++ {
		t := args[i]
		if t == "--" {
			out.passthrough = append(out.passthrough, args[i+1:]...)
			break
		}
		if t == "new" {
			if out.mode == "root" {
				out.mode = "new"
				continue
			}
			return out, fmt.Errorf("invalid usage")
		}
		if t == "rr" {
			if out.mode == "root" {
				out.mode = "rr"
				continue
			}
			return out, fmt.Errorf("invalid usage")
		}
		if t == "last" {
			if out.mode == "root" {
				out.mode = "last"
				continue
			}
			return out, fmt.Errorf("invalid usage")
		}
		if t == "config" {
			if out.mode != "root" || i+1 >= len(args) || args[i+1] != "init" {
				return out, fmt.Errorf("usage: agentlb config init")
			}
			out.mode = "config_init"
			i++
			continue
		}
		if t == "--cmd" {
			i++
			if i >= len(args) {
				return out, fmt.Errorf("--cmd requires a value")
			}
			out.cmd = args[i]
			continue
		}
		if t == "--login-cmd" {
			i++
			if i >= len(args) {
				return out, fmt.Errorf("--login-cmd requires a value")
			}
			out.loginCmd = args[i]
			continue
		}
		if strings.HasPrefix(t, "--cmd=") {
			out.cmd = strings.TrimPrefix(t, "--cmd=")
			continue
		}
		if strings.HasPrefix(t, "--login-cmd=") {
			out.loginCmd = strings.TrimPrefix(t, "--login-cmd=")
			continue
		}
		if strings.HasPrefix(t, "-") {
			return out, fmt.Errorf("unknown flag: %s", t)
		}
		if out.mode == "new" && out.alias == "" {
			out.alias = t
			continue
		}
		return out, fmt.Errorf("unexpected argument: %s", t)
	}
	if out.mode == "root" && out.alias != "" {
		return out, fmt.Errorf("unexpected alias")
	}
	return out, nil
}

func debugf(format string, args ...any) {
	if os.Getenv("AGENTLB_DEBUG") != "1" {
		return
	}
	eprintln("debug: " + fmt.Sprintf(format, args...))
}

func eprintln(msg string) {
	fmt.Fprintln(os.Stderr, msg)
}
