package treesitter

import (
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestTreeSitterPreProcessor_MetadataCaptured(t *testing.T) {
	t.Parallel()

	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`package main

// Add adds two numbers.
func (c *Calculator) Add(a, b int) int {
	return a + b
}
`)

	chunks, err := processor.Process(t.Context(), "calc.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	meta := chunks[0].Metadata
	require.NotNil(t, meta, "metadata should be present when AST processing succeeds")
	assert.Equal(t, "Add", meta["symbol_name"])
	assert.Equal(t, "method", meta["symbol_kind"])
	assert.Equal(t, "main", meta["package"])
	assert.Contains(t, meta["signature"], "func (c *Calculator) Add(a, b int) int")
	assert.Contains(t, meta["doc"], "Add adds two numbers.")
	assert.NotEmpty(t, meta["start_line"])
	assert.NotEmpty(t, meta["end_line"])
}

func TestTreeSitterPreProcessor_IncludesGodocComments(t *testing.T) {
	processor := NewDocumentProcessor(80, 0, false)

	// Test case with godoc-style comments
	content := []byte(`package main

// Add returns the sum of two integers.
// This is a godoc comment.
func Add(a, b int) int {
	return a + b
}

// Subtract returns the difference between two integers.
func Subtract(a, b int) int {
	return a - b
}
`)

	// Use small chunk size to force separate chunks
	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 2, "Expected 2 chunks (one per function)")

	// Check that the first chunk includes the godoc comment
	assert.Contains(t, chunks[0].Content, "// Add returns the sum of two integers.")
	assert.Contains(t, chunks[0].Content, "// This is a godoc comment.")
	assert.Contains(t, chunks[0].Content, "func Add(a, b int) int {")

	// Check that the second chunk includes its godoc comment
	assert.Contains(t, chunks[1].Content, "// Subtract returns the difference between two integers.")
	assert.Contains(t, chunks[1].Content, "func Subtract(a, b int) int {")
}

func TestTreeSitterPreProcessor_FunctionWithoutComment(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`package main

func Multiply(a, b int) int {
	return a * b
}
`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	// Should only contain the function, no extra comments
	assert.Contains(t, chunks[0].Content, "func Multiply(a, b int) int {")
	assert.NotContains(t, chunks[0].Content, "//")
}

func TestTreeSitterPreProcessor_MethodWithComment(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`package main

type Calculator struct{}

// Calculate performs a calculation.
func (c Calculator) Calculate(a, b int) int {
	return a + b
}
`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	// Should include the comment and the method
	assert.Contains(t, chunks[0].Content, "// Calculate performs a calculation.")
	assert.Contains(t, chunks[0].Content, "func (c Calculator) Calculate(a, b int) int {")
}

func TestTreeSitterPreProcessor_MultilineComment(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`package main

// Divide divides two numbers.
// It returns an error if the divisor is zero.
// This is a multi-line comment.
func Divide(a, b int) (int, error) {
	if b == 0 {
		return 0, fmt.Errorf("division by zero")
	}
	return a / b, nil
}
`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	// Should include all comment lines
	assert.Contains(t, chunks[0].Content, "// Divide divides two numbers.")
	assert.Contains(t, chunks[0].Content, "// It returns an error if the divisor is zero.")
	assert.Contains(t, chunks[0].Content, "// This is a multi-line comment.")
	assert.Contains(t, chunks[0].Content, "func Divide(a, b int) (int, error) {")
}

func TestTreeSitterPreProcessor_BlankLinesBetweenCommentAndFunction(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	// Test with more than one blank line between comment and function
	// This should NOT include the comment
	content := []byte(`package main

// This is a detached comment.


func Process() {
	// implementation
}
`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	// Should NOT include the detached comment (too many blank lines)
	assert.NotContains(t, chunks[0].Content, "This is a detached comment")
	assert.Contains(t, chunks[0].Content, "func Process() {")
}

func TestTreeSitterPreProcessor_AdjacentComment(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	// Test with no blank line between comment and function (most common godoc style)
	content := []byte(`package main

// Handler handles requests.
func Handler() {
	// implementation
}
`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	// Should include the adjacent comment
	assert.Contains(t, chunks[0].Content, "// Handler handles requests.")
	assert.Contains(t, chunks[0].Content, "func Handler() {")
}

func TestTreeSitterPreProcessor_MixedCommentsAndFunctions(t *testing.T) {
	processor := NewDocumentProcessor(80, 0, false)

	content := []byte(`package main

import "fmt"

// First function with comment.
func First() {
	fmt.Println("first")
}

func Second() {
	fmt.Println("second")
}

// Third function with a detailed comment.
// It has multiple lines.
func Third() {
	fmt.Println("third")
}
`)

	// Use small chunk size to force separate chunks
	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 3)

	// First function should have its comment
	assert.Contains(t, chunks[0].Content, "// First function with comment.")
	assert.Contains(t, chunks[0].Content, "func First() {")

	// Second function should not have a comment
	assert.NotContains(t, chunks[1].Content, "//")
	assert.Contains(t, chunks[1].Content, "func Second() {")

	// Third function should have its multi-line comment
	assert.Contains(t, chunks[2].Content, "// Third function with a detailed comment.")
	assert.Contains(t, chunks[2].Content, "// It has multiple lines.")
	assert.Contains(t, chunks[2].Content, "func Third() {")
}

func TestTreeSitterPreProcessor_ChunkSizeRespected(t *testing.T) {
	processor := NewDocumentProcessor(50, 0, false)

	content := []byte(`package main

// Small adds two numbers.
func Small(a, b int) int {
	return a + b
}

// Another small function.
func Another(x int) int {
	return x * 2
}
`)

	// Set a small chunk size to force separate chunks
	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)

	// With a small chunk size, functions should be in separate chunks
	assert.Greater(t, len(chunks), 1, "Expected multiple chunks due to size limit")
}

func TestTreeSitterPreProcessor_LargeFunctionExceedsChunkSize(t *testing.T) {
	processor := NewDocumentProcessor(50, 0, false)

	content := []byte(`package main

// ProcessData processes a large amount of data.
// This function is intentionally large.
func ProcessData() {
	// Line 1
	// Line 2
	// Line 3
	// Line 4
	// Line 5
	// Line 6
	// Line 7
	// Line 8
	// Line 9
	// Line 10
}
`)

	// Set chunk size smaller than the function
	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1, "Large function should be in its own chunk despite exceeding limit")

	// Should include the comment even though it's oversized
	assert.Contains(t, chunks[0].Content, "// ProcessData processes a large amount of data.")
	assert.Contains(t, chunks[0].Content, "func ProcessData() {")
}

func TestTreeSitterPreProcessor_UnsupportedExtension(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`console.log("hello");`)

	// For unsupported extensions, it falls back to text chunking
	chunks, err := processor.Process(t.Context(), "test.js", content)
	require.NoError(t, err)
	// Text fallback should produce chunks
	require.NotNil(t, chunks)
	require.Len(t, chunks, 1)
}

func TestTreeSitterPreProcessor_EmptyFile(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`package main`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	// Falls back to text chunking for files with no functions
	require.NotEmpty(t, chunks)
}

func TestTreeSitterPreProcessor_BlockCommentStyle(t *testing.T) {
	processor := NewDocumentProcessor(1000, 0, false)

	content := []byte(`package main

/*
Block comment style.
Multiple lines.
*/
func BlockCommented() {
	return
}
`)

	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1)

	// Should include block comment
	assert.Contains(t, chunks[0].Content, "/*")
	assert.Contains(t, chunks[0].Content, "Block comment style.")
	assert.Contains(t, chunks[0].Content, "func BlockCommented() {")
}

func TestTreeSitterPreProcessor_FunctionsGroupedInChunk(t *testing.T) {
	processor := NewDocumentProcessor(10000, 0, false)

	content := []byte(`package main

// A is small.
func A() int {
	return 1
}

// B is small.
func B() int {
	return 2
}

// C is small.
func C() int {
	return 3
}
`)

	// Large chunk size should group all functions together
	chunks, err := processor.Process(t.Context(), "test.go", content)
	require.NoError(t, err)
	require.Len(t, chunks, 1, "Expected all small functions to be grouped in one chunk")

	// Should contain all functions with their comments
	assert.Contains(t, chunks[0].Content, "// A is small.")
	assert.Contains(t, chunks[0].Content, "func A() int {")
	assert.Contains(t, chunks[0].Content, "// B is small.")
	assert.Contains(t, chunks[0].Content, "func B() int {")
	assert.Contains(t, chunks[0].Content, "// C is small.")
	assert.Contains(t, chunks[0].Content, "func C() int {")
}
