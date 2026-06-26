package chunk

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
	"strings"
)

// Chunk represents a piece of text from a document
type Chunk struct {
	Index    int
	Content  string
	Metadata map[string]string
}

// DocumentProcessor takes file content and returns chunks.
// Config (size, overlap, etc.) is set at construction time.
type DocumentProcessor interface {
	Process(ctx context.Context, path string, content []byte) ([]Chunk, error)
}

// TextDocumentProcessor is the default text-based chunker
type TextDocumentProcessor struct {
	size                  int
	overlap               int
	respectWordBoundaries bool
}

// NewTextDocumentProcessor creates a text-based document processor
func NewTextDocumentProcessor(size, overlap int, respectWordBoundaries bool) *TextDocumentProcessor {
	if size <= 0 {
		size = 1000
	}
	overlap = max(overlap, 0)
	if overlap >= size {
		overlap = size / 2
	}
	return &TextDocumentProcessor{
		size:                  size,
		overlap:               overlap,
		respectWordBoundaries: respectWordBoundaries,
	}
}

// Process implements DocumentProcessor for text-based chunking
func (t *TextDocumentProcessor) Process(_ context.Context, _ string, content []byte) ([]Chunk, error) {
	return t.chunkText(string(content)), nil
}

// chunkText splits text into overlapping chunks
func (t *TextDocumentProcessor) chunkText(text string) []Chunk {
	var chunks []Chunk
	runes := []rune(text)
	totalLen := len(runes)

	if totalLen == 0 {
		return chunks
	}

	index := 0
	start := 0

	for start < totalLen {
		// Calculate end position (start + size, but not beyond document end)
		end := min(start+t.size, totalLen)

		// If respecting word boundaries and we're NOT on the final chunk,
		// try to adjust the end so that we don't cut in the middle of a word.
		// For the final chunk (end == totalLen) we always take the remainder
		// of the document as-is to avoid generating progressively smaller
		// tail chunks.
		if t.respectWordBoundaries && end > start && end < totalLen {
			// Limit search to the current chunk window.
			target := end

			// Backtrack from target to find whitespace; if none is found
			// in a reasonable range, keep the original end so that we
			// still make progress even for very long "words".
			searchEnd := t.findNearestWhitespace(runes[start:target+1], target-start) + start
			if searchEnd > start && searchEnd < end {
				end = searchEnd
			}
		}

		// Create chunk
		chunk := string(runes[start:end])
		chunks = append(chunks, Chunk{
			Index:   index,
			Content: strings.TrimSpace(chunk),
		})

		index++

		// If we've reached the end of the document, we're done
		if end >= totalLen {
			break
		}

		// Next chunk starts at the end of the previous chunk minus overlap
		nextStart := end - t.overlap

		// CRITICAL: Ensure we always make forward progress
		// If nextStart would move us backward or keep us in place, advance by at least 1
		if nextStart <= start {
			nextStart = start + 1
		}

		// When respecting word boundaries, make sure the next chunk
		// does not start in the middle of a word. Move the start
		// forward to the next whitespace, then to the next non-whitespace.
		if t.respectWordBoundaries {
			// Move forward until we hit whitespace or end-of-text
			for nextStart < totalLen && !t.isWhitespace(runes[nextStart]) {
				nextStart++
			}
			// Skip the whitespace itself so we start at the first character
			// of the next word (if any).
			for nextStart < totalLen && t.isWhitespace(runes[nextStart]) {
				nextStart++
			}
		}

		start = nextStart
	}

	return chunks
}

// findNearestWhitespace finds the nearest whitespace boundary to the target position
// It searches backward first (within a reasonable distance), then forward if needed
func (t *TextDocumentProcessor) findNearestWhitespace(runes []rune, target int) int {
	// Don't search beyond 20% of the total length in either direction
	maxSearchDistance := len(runes) / 5
	maxSearchDistance = max(maxSearchDistance, 50)
	maxSearchDistance = min(maxSearchDistance, 500)

	// Search backward first (prefer to keep chunks slightly smaller)
	for i := 0; i < maxSearchDistance && target-i > 0; i++ {
		pos := target - i
		if t.isWhitespace(runes[pos]) {
			// Skip consecutive whitespace
			for pos > 0 && t.isWhitespace(runes[pos-1]) {
				pos--
			}
			return pos
		}
	}

	// Search forward if no whitespace found backward
	for i := 1; i < maxSearchDistance && target+i < len(runes); i++ {
		pos := target + i
		if t.isWhitespace(runes[pos]) {
			return pos
		}
	}

	// If no whitespace found in search range, return original target
	return target
}

// isWhitespace checks if a rune is whitespace
func (t *TextDocumentProcessor) isWhitespace(r rune) bool {
	return r == ' ' || r == '\t' || r == '\n' || r == '\r'
}

// --- File utility functions (standalone, not tied to any processor) ---

// ProcessFile reads a file and processes it using the given document processor
func ProcessFile(ctx context.Context, dp DocumentProcessor, path string) ([]Chunk, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read file: %w", err)
	}
	return dp.Process(ctx, path, content)
}

// FileHash calculates SHA256 hash of a file
func FileHash(path string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", fmt.Errorf("failed to open file: %w", err)
	}
	defer f.Close()

	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", fmt.Errorf("failed to hash file: %w", err)
	}

	return hex.EncodeToString(h.Sum(nil)), nil
}
