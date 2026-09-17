package chat

import (
	"strings"
	"unicode"
)

// Citation is a source the provider grounded a response on (e.g. Google
// Search grounding or a quoted web page). Display-only: it is persisted on
// the assistant message for transcripts but never replayed to the model.
type Citation struct {
	URI   string `json:"uri"`
	Title string `json:"title,omitempty"`
}

// ServerToolCall records one complete built-in tool invocation the provider
// ran on its own side (e.g. Gemini code execution or a Google Search query).
// Display-only: the runtime never executes it, never re-sends it to the
// provider, and never turns it into a tools.ToolCall.
type ServerToolCall struct {
	// Name identifies the built-in tool, e.g. "code_execution" or "google_search".
	Name string `json:"name"`
	// Language is the source language for code inputs (lowercase, e.g. "python").
	Language string `json:"language,omitempty"`
	Input    string `json:"input,omitempty"`
	Output   string `json:"output,omitempty"`
	IsError  bool   `json:"is_error,omitempty"`
}

// MergeCitations appends the citations of add whose URI is not yet present in
// dst, preserving first-seen order, and returns the grown slice with the
// newly added entries. Entries are sanitized first (see [SanitizeCitation]);
// unusable ones are dropped.
func MergeCitations(dst, add []Citation) (merged, added []Citation) {
	seen := make(map[string]struct{}, len(dst))
	for _, c := range dst {
		seen[c.URI] = struct{}{}
	}
	for _, c := range add {
		c, ok := SanitizeCitation(c)
		if !ok {
			continue
		}
		if _, dup := seen[c.URI]; dup {
			continue
		}
		seen[c.URI] = struct{}{}
		dst = append(dst, c)
		added = append(added, c)
	}
	return dst, added
}

// maxCitationURIBytes bounds a provider-supplied source URI before it is
// persisted or displayed.
const maxCitationURIBytes = 2048

// IsDisplayControl reports whether r must not reach a terminal or transcript
// verbatim: C0/C1 control characters and DEL, plus the Unicode
// bidirectional formatting characters that can visually reorder text.
func IsDisplayControl(r rune) bool {
	switch {
	case unicode.IsControl(r):
		return true
	case r == 0x061C, r == 0x200E, r == 0x200F:
		return true
	case r >= 0x202A && r <= 0x202E, r >= 0x2066 && r <= 0x2069:
		return true
	}
	return false
}

// SanitizeCitation neutralizes provider-supplied citation fields before they
// reach the transcript or a terminal. The URI is untrusted display data: it
// is rejected when empty, overlong, or carrying whitespace, control or bidi
// characters (none of which belong in a URI). The title is trimmed, has such
// characters and angle brackets rewritten, and is capped at
// [MaxSanitizedFieldBytes].
func SanitizeCitation(c Citation) (Citation, bool) {
	uri := strings.TrimSpace(c.URI)
	if uri == "" || len(uri) > maxCitationURIBytes || strings.ContainsFunc(uri, func(r rune) bool {
		return unicode.IsSpace(r) || r == '<' || r == '>' || IsDisplayControl(r)
	}) {
		return Citation{}, false
	}
	var title strings.Builder
	for _, r := range strings.TrimSpace(c.Title) {
		if r == '<' || r == '>' || IsDisplayControl(r) {
			r = '_'
		}
		title.WriteRune(r)
	}
	return Citation{URI: uri, Title: TruncateUTF8Bytes(title.String(), MaxSanitizedFieldBytes)}, true
}
