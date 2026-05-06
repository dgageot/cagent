package secretsscan

import (
	"testing"

	"github.com/stretchr/testify/assert"
)

// TestAhoCorasickOverlappingPatterns verifies that the AC automaton
// correctly detects all overlapping patterns in a single scan. This
// is a regression guard for the fail-link computation in
// buildAhoCorasick: if the BFS-based table construction incorrectly
// inherits transitions, suffix patterns can be missed.
func TestAhoCorasickOverlappingPatterns(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name     string
		patterns []string
		text     string
		expected []int // indices of patterns that should match
	}{
		{
			name:     "overlapping at boundary",
			patterns: []string{"ab", "bc"},
			text:     "abc",
			expected: []int{0, 1},
		},
		{
			name:     "suffix patterns",
			patterns: []string{"he", "she", "his", "hers"},
			text:     "ushers",
			expected: []int{0, 1, 3}, // "he" in "ushers", "she" in "ushers", "hers" in "ushers"
		},
		{
			name:     "nested patterns",
			patterns: []string{"abc", "bc", "c"},
			text:     "abc",
			expected: []int{0, 1, 2},
		},
		{
			name:     "repeated patterns",
			patterns: []string{"a", "aa", "aaa"},
			text:     "aaa",
			expected: []int{0, 1, 2},
		},
		{
			name:     "no match",
			patterns: []string{"xyz", "foo"},
			text:     "bar",
			expected: nil,
		},
		{
			name:     "case folding",
			patterns: []string{"key"},
			text:     "KEY",
			expected: []int{0},
		},
		{
			name:     "multiple occurrences",
			patterns: []string{"key"},
			text:     "key and key again",
			expected: []int{0},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			ac := buildAhoCorasick(tt.patterns)
			mask := ac.scan(tt.text)

			var found []int
			for i := range len(tt.patterns) {
				if mask[i>>6]&(1<<uint(i&63)) != 0 {
					found = append(found, i)
				}
			}

			assert.Equal(t, tt.expected, found,
				"patterns %v in text %q", tt.patterns, tt.text)
		})
	}
}

// TestAhoCorasickPanicOnTooManyPatterns verifies that buildAhoCorasick
// panics when given more than 128 patterns, which would overflow the
// kwMask bitset.
func TestAhoCorasickPanicOnTooManyPatterns(t *testing.T) {
	t.Parallel()

	patterns := make([]string, 129)
	for i := range patterns {
		patterns[i] = string(rune('a' + i%26))
	}

	assert.PanicsWithValue(t, "secretsscan: too many AC patterns for kwMask",
		func() { buildAhoCorasick(patterns) })
}

// TestKwMaskOperations verifies the kwMask bitset operations.
func TestKwMaskOperations(t *testing.T) {
	t.Parallel()

	var m kwMask
	assert.True(t, m.empty(), "zero-initialized mask should be empty")

	m.set(0)
	assert.False(t, m.empty(), "mask with bit 0 set should not be empty")
	assert.Equal(t, uint64(1), m[0], "bit 0 should be set in first word")

	m.set(63)
	assert.Equal(t, uint64(1<<63|1), m[0], "bit 63 should be set in first word")

	m.set(64)
	assert.Equal(t, uint64(1), m[1], "bit 64 should be set in second word")

	m.set(127)
	assert.Equal(t, uint64(1<<63|1), m[1], "bit 127 should be set in second word")

	var other kwMask
	other.set(0)
	assert.True(t, m.overlaps(other), "masks with shared bit should overlap")

	other = kwMask{}
	other.set(100)
	assert.False(t, m.overlaps(other), "masks with no shared bits should not overlap")
}
