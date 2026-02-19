package session

import "testing"

func TestValidateAlias(t *testing.T) {
	pattern := `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`
	cases := []struct {
		alias string
		ok    bool
	}{
		{"a1", true},
		{"abc.DEF-1", true},
		{"", false},
		{"-bad", false},
		{"../../x", false},
	}
	for _, tc := range cases {
		err := ValidateAlias(tc.alias, pattern)
		if tc.ok && err != nil {
			t.Fatalf("expected valid alias %q, got %v", tc.alias, err)
		}
		if !tc.ok && err == nil {
			t.Fatalf("expected invalid alias %q", tc.alias)
		}
	}
}
