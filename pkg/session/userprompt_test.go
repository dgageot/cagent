package session

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestAddPromptFileContent(t *testing.T) {
	tests := []struct {
		name         string
		promptFile   string
		homeContent  string
		workContent  string
		setupFunc    func(tmpHome, tmpDir string) error
		expectEmpty  bool
		expectError  bool
		expectedText string
	}{
		{
			name:        "empty prompt file",
			promptFile:  "",
			expectEmpty: true,
		},
		{
			name:        "nonexistent files",
			promptFile:  "nonexistent.txt",
			expectEmpty: true,
		},
		{
			name:        "only home file exists",
			promptFile:  "test.md",
			homeContent: "Home content",
			setupFunc: func(tmpHome, tmpDir string) error {
				return os.WriteFile(filepath.Join(tmpHome, "test.md"), []byte("Home content"), 0644)
			},
			expectedText: "Home content",
		},
		{
			name:        "only work file exists",
			promptFile:  "test.md",
			workContent: "Work content",
			setupFunc: func(tmpHome, tmpDir string) error {
				return os.WriteFile(filepath.Join(tmpDir, "test.md"), []byte("Work content"), 0644)
			},
			expectedText: "Work content",
		},
		{
			name:        "both files exist",
			promptFile:  "test.md",
			homeContent: "Home content",
			workContent: "Work content",
			setupFunc: func(tmpHome, tmpDir string) error {
				if err := os.WriteFile(filepath.Join(tmpHome, "test.md"), []byte("Home content"), 0644); err != nil {
					return err
				}
				return os.WriteFile(filepath.Join(tmpDir, "test.md"), []byte("Work content"), 0644)
			},
			expectedText: "Home content\nWork content",
		},
		{
			name:       "empty files",
			promptFile: "empty.txt",
			setupFunc: func(tmpHome, tmpDir string) error {
				if err := os.WriteFile(filepath.Join(tmpHome, "empty.txt"), []byte(""), 0644); err != nil {
					return err
				}
				return os.WriteFile(filepath.Join(tmpDir, "empty.txt"), []byte(""), 0644)
			},
			expectEmpty: true, // Empty files should result in empty output
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			// Create fresh temp directories for each test
			tmpDir := t.TempDir()
			tmpHome := t.TempDir()

			// Set HOME environment variable to our temp home directory
			t.Setenv("HOME", tmpHome)

			if tt.setupFunc != nil {
				require.NoError(t, tt.setupFunc(tmpHome, tmpDir))
			}

			result, err := AddPromptFileContent(tmpDir, tt.promptFile)

			if tt.expectError {
				assert.Error(t, err)
				return
			}

			require.NoError(t, err)

			if tt.expectEmpty {
				assert.Empty(t, result)
				return
			}

			expectedHeader := "\n\n# Project-Specific Context\n Make sure to follow the instructions in the context below\n"
			expected := expectedHeader + tt.expectedText

			assert.Equal(t, expected, result)
			assert.True(t, strings.HasPrefix(result, "\n\n"))
			assert.Contains(t, result, "# Project-Specific Context")
		})
	}
}

func TestAddPromptFileContentEdgeCases(t *testing.T) {
	t.Run("invalid working directory", func(t *testing.T) {
		tmpHome := t.TempDir()
		t.Setenv("HOME", tmpHome)

		result, err := AddPromptFileContent("/nonexistent/directory", "prompt.txt")
		assert.Empty(t, result)
		assert.NoError(t, err) // Should not error for file not found
	})

	t.Run("directory instead of file in work dir", func(t *testing.T) {
		tmpHome := t.TempDir()
		t.Setenv("HOME", tmpHome)

		tmpDir := t.TempDir()
		require.NoError(t, os.Mkdir(filepath.Join(tmpDir, "dir"), 0755))

		result, err := AddPromptFileContent(tmpDir, "dir")
		assert.Empty(t, result)
		assert.Error(t, err) // Should error for non-file-not-found errors
	})

	t.Run("directory instead of file in home dir", func(t *testing.T) {
		tmpHome := t.TempDir()
		t.Setenv("HOME", tmpHome)

		tmpDir := t.TempDir()
		require.NoError(t, os.Mkdir(filepath.Join(tmpHome, "dir"), 0755))

		result, err := AddPromptFileContent(tmpDir, "dir")
		assert.Empty(t, result)
		assert.Error(t, err) // Should error for non-file-not-found errors
	})
}