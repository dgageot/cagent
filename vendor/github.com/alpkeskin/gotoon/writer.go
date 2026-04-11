package gotoon

import "strings"

// LineWriter manages indented line output for TOON format
type LineWriter struct {
	lines            []string
	indentationString string
}

// NewLineWriter creates a new LineWriter with the specified indentation size
func NewLineWriter(indentSize int) *LineWriter {
	return &LineWriter{
		lines:             make([]string, 0),
		indentationString: strings.Repeat(" ", indentSize),
	}
}

// Push adds a new line with the specified depth and content
func (w *LineWriter) Push(depth int, content string) {
	indent := strings.Repeat(w.indentationString, depth)
	w.lines = append(w.lines, indent+content)
}

// String returns the accumulated lines joined with newlines
func (w *LineWriter) String() string {
	return strings.Join(w.lines, "\n")
}
