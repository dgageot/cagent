package session

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// AddPromptFileContent reads a prompt file from both the user's home directory and
// the working directory, concatenating them with home content first. Returns an
// empty string if neither file exists. Non-file-not-found errors are returned.
func AddPromptFileContent(workingDir, promptFile string) (string, error) {
	if promptFile == "" {
		return "", nil
	}

	var contents []string

	// Try to read from home directory first
	homeDir, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("failed to get user home directory: %w", err)
	}

	homeContent, err := readPromptFile(filepath.Join(homeDir, promptFile))
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return "", fmt.Errorf("failed to read prompt file from home directory: %w", err)
	}
	if homeContent != "" {
		contents = append(contents, homeContent)
	}

	// Try to read from working directory
	workContent, err := readPromptFile(filepath.Join(workingDir, promptFile))
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return "", fmt.Errorf("failed to read prompt file from working directory: %w", err)
	}
	if workContent != "" {
		contents = append(contents, workContent)
	}

	// Return empty string if no content found
	if len(contents) == 0 {
		return "", nil
	}

	// Concatenate all content
	combinedContent := strings.Join(contents, "\n")
	return "\n\n" + fmt.Sprintf("# Project-Specific Context\n Make sure to follow the instructions in the context below\n%s", combinedContent), nil
}

// readPromptFile reads a single prompt file and returns its content.
// Returns empty string and os.ErrNotExist if file doesn't exist.
func readPromptFile(filePath string) (string, error) {
	buf, err := os.ReadFile(filePath)
	if err != nil {
		return "", err
	}
	return string(buf), nil
}
