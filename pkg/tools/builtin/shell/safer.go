package shell

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"regexp"
	"strings"
	"sync"

	"github.com/docker/docker-agent/pkg/tools"
)

//go:embed safety_patterns.json
var safetyPatternsJSON []byte

type safetyPattern struct {
	Pattern     string
	BlastRadius tools.BlastRadiusLevel
	regexp      *regexp.Regexp
}

type safetyPatternEntry struct {
	Pattern     string `json:"pattern"`
	BlastRadius string `json:"blast_radius"`
}

var loadSafetyPatterns = sync.OnceValues(func() ([]safetyPattern, error) {
	var root any
	if err := json.Unmarshal(safetyPatternsJSON, &root); err != nil {
		return nil, fmt.Errorf("parse shell safety patterns: %w", err)
	}

	entries := collectSafetyPatternEntries(root)
	patterns := make([]safetyPattern, 0, len(entries))
	for _, entry := range entries {
		pattern := normalizeCommand(entry.Pattern)
		re, err := regexp.Compile(patternToRegexp(pattern))
		if err != nil {
			return nil, fmt.Errorf("compile shell safety pattern %q: %w", entry.Pattern, err)
		}
		patterns = append(patterns, safetyPattern{
			Pattern:     entry.Pattern,
			BlastRadius: blastRadiusLevel(entry.BlastRadius),
			regexp:      re,
		})
	}
	return patterns, nil
})

func collectSafetyPatternEntries(value any) []safetyPatternEntry {
	switch v := value.(type) {
	case []any:
		var entries []safetyPatternEntry
		for _, item := range v {
			entries = append(entries, collectSafetyPatternEntries(item)...)
		}
		return entries
	case map[string]any:
		if pattern, ok := v["pattern"].(string); ok {
			if blastRadius, ok := v["blast_radius"].(string); ok {
				return []safetyPatternEntry{{Pattern: pattern, BlastRadius: blastRadius}}
			}
		}
		var entries []safetyPatternEntry
		for _, item := range v {
			entries = append(entries, collectSafetyPatternEntries(item)...)
		}
		return entries
	default:
		return nil
	}
}

func patternToRegexp(pattern string) string {
	var b strings.Builder
	b.WriteString(`(?i)(?:^|.*\b)`)
	for i := 0; i < len(pattern); {
		switch pattern[i] {
		case '<':
			if end := strings.IndexByte(pattern[i:], '>'); end >= 0 {
				b.WriteString(`\S+`)
				i += end + 1
				continue
			}
		case '.':
			if strings.HasPrefix(pattern[i:], "...") {
				b.WriteString(`.*`)
				i += len("...")
				continue
			}
		}
		b.WriteString(regexp.QuoteMeta(string(pattern[i])))
		i++
	}
	b.WriteString(`(?:$|\b.*)`)
	return b.String()
}

func blastRadiusLevel(raw string) tools.BlastRadiusLevel {
	switch strings.ToUpper(strings.TrimSpace(raw)) {
	case "LOW":
		return tools.BlastRadiusLow
	case "MEDIUM", "LOW-MEDIUM":
		return tools.BlastRadiusMedium
	case "HIGH", "MEDIUM-HIGH":
		return tools.BlastRadiusHigh
	default:
		return tools.BlastRadiusUnknown
	}
}

func assessDestructiveShellCommand(command string) *tools.ToolCallSafety {
	patterns, err := loadSafetyPatterns()
	if err != nil {
		return &tools.ToolCallSafety{
			Destructive: true,
			BlastRadius: tools.BlastRadiusUnknown,
			Reason:      err.Error(),
		}
	}

	normalized := normalizeCommand(command)
	var best *tools.ToolCallSafety
	bestSeverity := 0
	for _, pattern := range patterns {
		if !pattern.regexp.MatchString(normalized) {
			continue
		}
		severity := blastRadiusSeverity(pattern.BlastRadius)
		if severity <= bestSeverity {
			continue
		}
		bestSeverity = severity
		best = &tools.ToolCallSafety{
			Destructive: true,
			BlastRadius: pattern.BlastRadius,
			Reason:      "Command matches destructive operation: " + pattern.Pattern,
		}
	}
	return best
}

func blastRadiusSeverity(level tools.BlastRadiusLevel) int {
	switch level {
	case tools.BlastRadiusHigh:
		return 4
	case tools.BlastRadiusUnknown:
		return 3
	case tools.BlastRadiusMedium:
		return 2
	case tools.BlastRadiusLow:
		return 1
	default:
		return 0
	}
}

func normalizeCommand(command string) string {
	return strings.Join(strings.Fields(strings.ToLower(command)), " ")
}
