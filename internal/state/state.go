package state

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"time"

	"agentlb/internal/config"
	"github.com/gofrs/flock"
)

type SessionState struct {
	Alias             string  `json:"alias"`
	CreatedAt         string  `json:"created_at"`
	LastUsedAt        string  `json:"last_used_at,omitempty"`
	RecentAssignments []int64 `json:"recent_assignments,omitempty"`
}

type GlobalState struct {
	RoundRobinIndex int    `json:"round_robin_index"`
	LastAlias       string `json:"last_alias,omitempty"`
}

type Store struct {
	Root         string
	ConfigPath   string
	SessionsDir  string
	StateDir     string
	SessionState string
	GlobalState  string
	LockPath     string
}

func NewStore() (*Store, error) {
	root, err := config.AgentLBRoot()
	if err != nil {
		return nil, err
	}
	cp, err := config.ConfigPath()
	if err != nil {
		return nil, err
	}
	st := &Store{
		Root:         root,
		ConfigPath:   cp,
		SessionsDir:  filepath.Join(root, "sessions"),
		StateDir:     filepath.Join(root, "state"),
		SessionState: filepath.Join(root, "state", "sessions"),
		GlobalState:  filepath.Join(root, "state", "global.json"),
		LockPath:     filepath.Join(root, "locks", "state.lock"),
	}
	return st, nil
}

func (s *Store) EnsureLayout() error {
	dirs := []string{s.Root, s.SessionsDir, s.StateDir, s.SessionState, filepath.Dir(s.LockPath)}
	for _, d := range dirs {
		if err := os.MkdirAll(d, 0o700); err != nil {
			return err
		}
		if err := os.Chmod(d, 0o700); err != nil {
			return err
		}
	}
	f, err := os.OpenFile(s.LockPath, os.O_CREATE, 0o600)
	if err != nil {
		return err
	}
	_ = f.Close()
	return os.Chmod(s.LockPath, 0o600)
}

func (s *Store) Lock() (func(), error) {
	fl := flock.New(s.LockPath)
	ok, err := fl.TryLock()
	if err != nil {
		return nil, err
	}
	if !ok {
		if err := fl.Lock(); err != nil {
			return nil, err
		}
	}
	return func() { _ = fl.Unlock() }, nil
}

func (s *Store) LoadGlobal() (GlobalState, error) {
	var g GlobalState
	b, err := os.ReadFile(s.GlobalState)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return g, nil
		}
		return g, err
	}
	if err := json.Unmarshal(b, &g); err != nil {
		return g, err
	}
	return g, nil
}

func (s *Store) SaveGlobal(g GlobalState) error {
	return s.writeJSON(s.GlobalState, g)
}

func (s *Store) SessionDir(alias string) string {
	return filepath.Join(s.SessionsDir, alias)
}

func (s *Store) SessionStatePath(alias string) string {
	return filepath.Join(s.SessionState, alias+".json")
}

func (s *Store) ListAliases() ([]string, error) {
	ents, err := os.ReadDir(s.SessionsDir)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil, nil
		}
		return nil, err
	}
	var aliases []string
	for _, e := range ents {
		if e.IsDir() {
			aliases = append(aliases, e.Name())
		}
	}
	sort.Strings(aliases)
	return aliases, nil
}

func (s *Store) LoadSession(alias string) (SessionState, error) {
	var ss SessionState
	path := s.SessionStatePath(alias)
	b, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			ss = SessionState{Alias: alias, CreatedAt: time.Now().UTC().Format(time.RFC3339)}
			return ss, nil
		}
		return ss, err
	}
	if err := json.Unmarshal(b, &ss); err != nil {
		return ss, err
	}
	if ss.Alias == "" {
		ss.Alias = alias
	}
	if ss.CreatedAt == "" {
		ss.CreatedAt = time.Now().UTC().Format(time.RFC3339)
	}
	return ss, nil
}

func (s *Store) SaveSession(ss SessionState) error {
	if ss.Alias == "" {
		return fmt.Errorf("empty alias")
	}
	if ss.CreatedAt == "" {
		ss.CreatedAt = time.Now().UTC().Format(time.RFC3339)
	}
	return s.writeJSON(s.SessionStatePath(ss.Alias), ss)
}

func (s *Store) RecordAssignment(alias string, spreadWindow int) error {
	ss, err := s.LoadSession(alias)
	if err != nil {
		return err
	}
	now := time.Now().UTC()
	ss.LastUsedAt = now.Format(time.RFC3339)
	ss.RecentAssignments = append(ss.RecentAssignments, now.Unix())
	if spreadWindow > 0 && len(ss.RecentAssignments) > spreadWindow {
		ss.RecentAssignments = ss.RecentAssignments[len(ss.RecentAssignments)-spreadWindow:]
	}
	return s.SaveSession(ss)
}

func (s *Store) writeJSON(path string, v any) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	b, err := json.MarshalIndent(v, "", "  ")
	if err != nil {
		return err
	}
	if err := os.WriteFile(path, b, 0o600); err != nil {
		return err
	}
	return os.Chmod(path, 0o600)
}
