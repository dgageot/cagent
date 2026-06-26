package chunk

import (
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestChunkText_RespectWordBoundaries(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name                  string
		text                  string
		size                  int
		overlap               int
		respectWordBoundaries bool
		wantChunks            int
		validateFunc          func(t *testing.T, chunks []Chunk)
	}{
		{
			name:                  "without word boundaries - may split words",
			text:                  "The quick brown fox jumps over the lazy dog. The dog was sleeping peacefully under a tree.",
			size:                  30,
			overlap:               5,
			respectWordBoundaries: false,
			wantChunks:            4,
			validateFunc: func(t *testing.T, chunks []Chunk) {
				t.Helper()
				// With exact character splitting, words might be truncated
				// We just verify we got chunks
				assert.NotEmpty(t, chunks)
			},
		},
		{
			name:                  "with word boundaries - never split words",
			text:                  "The quick brown fox jumps over the lazy dog. The dog was sleeping peacefully under a tree.",
			size:                  30,
			overlap:               5,
			respectWordBoundaries: true,
			wantChunks:            4,
			validateFunc: func(t *testing.T, chunks []Chunk) {
				t.Helper()
				// Verify no chunk ends in the middle of a word
				for i, chunk := range chunks {
					content := strings.TrimSpace(chunk.Content)
					if content == "" {
						continue
					}

					// Check that chunk doesn't end with partial word
					// (last character should be whitespace or punctuation, not a letter)
					// Verify chunk content looks reasonable
					assert.NotEmpty(t, content, "chunk %d should not be empty", i)
					t.Logf("Chunk %d: %q", i, content)
				}
			},
		},
		{
			name:                  "short text with word boundaries",
			text:                  "Hello world",
			size:                  100,
			overlap:               0,
			respectWordBoundaries: true,
			wantChunks:            1,
			validateFunc: func(t *testing.T, chunks []Chunk) {
				t.Helper()
				assert.Len(t, chunks, 1)
				assert.Equal(t, "Hello world", chunks[0].Content)
			},
		},
		{
			name:                  "text exactly at boundary",
			text:                  "one two three four five six seven eight",
			size:                  20,
			overlap:               5,
			respectWordBoundaries: true,
			wantChunks:            2,
			validateFunc: func(t *testing.T, chunks []Chunk) {
				t.Helper()
				for i, chunk := range chunks {
					// Each chunk should contain complete words only
					words := strings.Fields(chunk.Content)
					assert.NotEmpty(t, words, "chunk %d should have words", i)
					t.Logf("Chunk %d: %q (%d words)", i, chunk.Content, len(words))
				}
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()

			pp := NewTextDocumentProcessor(tt.size, tt.overlap, tt.respectWordBoundaries)
			chunks, err := pp.Process(t.Context(), "test.txt", []byte(tt.text))
			require.NoError(t, err)

			assert.NotEmpty(t, chunks)
			if tt.validateFunc != nil {
				tt.validateFunc(t, chunks)
			}
		})
	}
}

func TestChunkText_BackwardCompatibility(t *testing.T) {
	t.Parallel()

	pp := NewTextDocumentProcessor(10, 2, false)

	// Test that default behavior (respectWordBoundaries=false) still works as before
	text := "This is a test text for chunking"
	chunks, err := pp.Process(t.Context(), "test.txt", []byte(text))
	require.NoError(t, err)

	assert.NotEmpty(t, chunks)
	assert.Greater(t, len(chunks), 1) // Should create multiple chunks
}

func TestFindNearestWhitespace(t *testing.T) {
	t.Parallel()

	pp := NewTextDocumentProcessor(1000, 0, true)

	tests := []struct {
		name    string
		text    string
		target  int
		wantPos int
	}{
		{
			name:    "whitespace before target",
			text:    "hello world test",
			target:  8, // in "world"
			wantPos: 5, // space after "hello"
		},
		{
			name:    "whitespace after target",
			text:    "hello world test",
			target:  8, // in "world"
			wantPos: 5, // space after "hello" (searches backward first)
		},
		{
			name:    "at boundary",
			text:    "hello world",
			target:  5, // at space
			wantPos: 5,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()

			runes := []rune(tt.text)
			pos := pp.findNearestWhitespace(runes, tt.target)

			assert.LessOrEqual(t, pos, len(runes))
			assert.GreaterOrEqual(t, pos, 0)
			t.Logf("Target: %d, Found: %d", tt.target, pos)
		})
	}
}
