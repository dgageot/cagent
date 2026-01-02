package builtin

import (
	"context"
	"encoding/json"
	"fmt"
	"io/fs"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/bmatcuk/doublestar/v4"

	"github.com/docker/cagent/pkg/fsx"
	"github.com/docker/cagent/pkg/tools"
)

const (
	ToolNameReadFile               = "read_file"
	ToolNameReadMultipleFiles      = "read_multiple_files"
	ToolNameEditFile               = "edit_file"
	ToolNameWriteFile              = "write_file"
	ToolNameListAllowedDirectories = "list_allowed_directories"
	ToolNameAddAllowedDirectory    = "add_allowed_directory"
	ToolNameDirectoryTree          = "directory_tree"
	ToolNameListDirectory          = "list_directory"
	ToolNameSearchFiles            = "search_files"
	ToolNameSearchFilesContent     = "search_files_content"
)

// PostEditConfig represents a post-edit command configuration
type PostEditConfig struct {
	Path string // File path pattern (glob-style)
	Cmd  string // Command to execute (with $path placeholder)
}

type FilesystemTool struct {
	tools.BaseToolSet

	allowedDirectories []string
	postEditCommands   []PostEditConfig
	ignoreVCS          bool
	repoMatchers       map[string]*fsx.VCSMatcher // map from repo root to matcher
}

var _ tools.ToolSet = (*FilesystemTool)(nil)

type FileSystemOpt func(*FilesystemTool)

func WithPostEditCommands(postEditCommands []PostEditConfig) FileSystemOpt {
	return func(t *FilesystemTool) {
		t.postEditCommands = postEditCommands
	}
}

func WithIgnoreVCS(ignoreVCS bool) FileSystemOpt {
	return func(t *FilesystemTool) {
		t.ignoreVCS = ignoreVCS
	}
}

func NewFilesystemTool(allowedDirectories []string, opts ...FileSystemOpt) *FilesystemTool {
	t := &FilesystemTool{
		allowedDirectories: allowedDirectories,
		repoMatchers:       make(map[string]*fsx.VCSMatcher),
	}
	for _, opt := range opts {
		opt(t)
	}

	// Initialize gitignore matchers if VCS ignoring is enabled
	if t.ignoreVCS {
		t.initGitignoreMatchers()
	}

	return t
}

func (t *FilesystemTool) Instructions() string {
	return `## Filesystem Tool Instructions

This toolset provides comprehensive filesystem operations with built-in security restrictions.

### Security Model
- All operations are restricted to allowed directories only
- Use list_allowed_directories to see available paths
- Subdirectories within allowed directories are accessible
- Use add_allowed_directory to request access to new directories (requires user consent)

### Directory Access Management
- If you need access to a directory outside the allowed list, use add_allowed_directory
- This will request user consent before expanding filesystem access
- Always provide a clear reason when requesting new directory access

### Common Patterns
- Always check if directories exist before creating files
- Prefer read_multiple_files for batch operations
- Use search_files_content for finding specific code or text

### Performance Tips
- Use read_multiple_files instead of multiple read_file calls
- Use directory_tree with max_depth to limit large traversals
- Use appropriate exclude patterns in search operations`
}

type DirectoryTreeArgs struct {
	Path string `json:"path" jsonschema:"The directory path to traverse"`
}

type AddAllowedDirectoryArgs struct {
	Path string `json:"path" jsonschema:"The directory path to add to allowed directories"`
}

type WriteFileArgs struct {
	Path    string `json:"path" jsonschema:"The file path to write"`
	Content string `json:"content" jsonschema:"The content to write to the file"`
}

type ReadMultipleFilesArgs struct {
	Paths []string `json:"paths" jsonschema:"Array of file paths to read"`
	JSON  bool     `json:"json,omitempty" jsonschema:"Whether to return the result as JSON"`
}

type ReadMultipleFilesMeta struct {
	Files []ReadFileMeta `json:"files"`
}

type SearchFilesArgs struct {
	Path            string   `json:"path" jsonschema:"The starting directory path"`
	Pattern         string   `json:"pattern" jsonschema:"The glob pattern to match file names against"`
	ExcludePatterns []string `json:"excludePatterns,omitempty" jsonschema:"Patterns to exclude from search"`
}

type SearchFilesContentArgs struct {
	Path            string   `json:"path" jsonschema:"The starting directory path"`
	Query           string   `json:"query" jsonschema:"The text or regex pattern to search for"`
	IsRegex         bool     `json:"is_regex,omitempty" jsonschema:"If true, treat query as regex; otherwise literal text"`
	ExcludePatterns []string `json:"excludePatterns,omitempty" jsonschema:"Patterns to exclude from search"`
}

type ListDirectoryArgs struct {
	Path string `json:"path" jsonschema:"The directory path to list"`
}

type ListDirectoryMeta struct {
	Files     []string `json:"files"`
	Dirs      []string `json:"dirs"`
	Truncated bool     `json:"truncated"`
}

type ReadFileArgs struct {
	Path string `json:"path" jsonschema:"The file path to read"`
}

type ReadFileMeta struct {
	Path      string `json:"path"`
	Content   string `json:"content"`
	LineCount int    `json:"lineCount"`
	Error     string `json:"error,omitempty"`
}

type Edit struct {
	OldText string `json:"oldText" jsonschema:"The exact text to replace"`
	NewText string `json:"newText" jsonschema:"The replacement text"`
}

type EditFileArgs struct {
	Path  string `json:"path" jsonschema:"The file path to edit"`
	Edits []Edit `json:"edits" jsonschema:"Array of edit operations"`
}

func (t *FilesystemTool) Tools(context.Context) ([]tools.Tool, error) {
	return []tools.Tool{
		{
			Name:        ToolNameDirectoryTree,
			Category:    "filesystem",
			Description: "Get a recursive tree view of files and directories as a JSON structure.",
			Parameters:  tools.MustSchemaFor[DirectoryTreeArgs](),
			// Manually define the schema here because
			// tools.MustSchemaFor(reflect.TypeFor[*TreeNode]()) doesn't support recursive types.
			OutputSchema: map[string]any{
				"type": "object",
				"properties": map[string]any{
					"name": map[string]any{
						"type":        "string",
						"description": "The name of the node",
					},
					"type": map[string]any{
						"type":        "string",
						"description": "The type of the node (file or directory)",
					},
					"children": map[string]any{
						"type":        "array",
						"description": "Optional list of child nodes",
						"items": map[string]any{
							"$ref": "#",
						},
					},
				},
				"required":             []string{"name", "type"},
				"additionalProperties": false,
			},
			Handler: NewHandler(t.handleDirectoryTree),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Directory Tree",
			},
		},
		{
			Name:         ToolNameEditFile,
			Category:     "filesystem",
			Description:  "Make line-based edits to a text file. Each edit replaces exact line sequences with new content.",
			Parameters:   tools.MustSchemaFor[EditFileArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handleEditFile),
			Annotations: tools.ToolAnnotations{
				Title: "Edit",
			},
		},
		{
			Name:                 ToolNameListAllowedDirectories,
			Category:             "filesystem",
			Description:          "Returns a list of directories that the server has permission to access. Don't call if you access only the current working directory. It's always allowed.",
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[[]string](),
			Handler:              NewHandler(t.handleListAllowedDirectories),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "List Allowed Directories",
			},
		},
		{
			Name:         ToolNameAddAllowedDirectory,
			Category:     "filesystem",
			Description:  "Request to add a new directory to the allowed directories list. This requires explicit user consent for security reasons.",
			Parameters:   tools.MustSchemaFor[AddAllowedDirectoryArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handleAddAllowedDirectory),
			Annotations: tools.ToolAnnotations{
				Title: "Add Allowed Directory",
			},
		},
		{
			Name:                 ToolNameListDirectory,
			Category:             "filesystem",
			Description:          "Get a detailed listing of all files and directories in a specified path.",
			Parameters:           tools.MustSchemaFor[ListDirectoryArgs](),
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[ListDirectoryMeta](),
			Handler:              NewHandler(t.handleListDirectory),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "List Directory",
			},
		},
		{
			Name:                 ToolNameReadFile,
			Category:             "filesystem",
			Description:          "Read the complete contents of a file from the file system.",
			Parameters:           tools.MustSchemaFor[ReadFileArgs](),
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[ReadFileMeta](),
			Handler:              NewHandler(t.handleReadFile),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Read",
			},
		},
		{
			Name:        ToolNameReadMultipleFiles,
			Category:    "filesystem",
			Description: "Read the contents of multiple files simultaneously.",
			Parameters:  tools.MustSchemaFor[ReadMultipleFilesArgs](),
			// TODO(dga): depends on the json param
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[ReadMultipleFilesMeta](),
			Handler:              NewHandler(t.handleReadMultipleFiles),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Read Multiple Files",
			},
		},
		{
			Name:         ToolNameSearchFiles,
			Category:     "filesystem",
			Description:  "Recursively search for files and directories matching a pattern. Prints the full paths of matching files and the total number of files found. The pattern syntax is the same as Go's filepath.Match.",
			Parameters:   tools.MustSchemaFor[SearchFilesArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handleSearchFiles),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Search Files",
			},
		},
		{
			Name:         ToolNameSearchFilesContent,
			Category:     "filesystem",
			Description:  "Searches for text or regex patterns in the content of files matching a GLOB pattern.",
			Parameters:   tools.MustSchemaFor[SearchFilesContentArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handleSearchFilesContent),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Search Files Content",
			},
		},
		{
			Name:         ToolNameWriteFile,
			Category:     "filesystem",
			Description:  "Create a new file or completely overwrite an existing file with new content.",
			Parameters:   tools.MustSchemaFor[WriteFileArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handleWriteFile),
			Annotations: tools.ToolAnnotations{
				Title: "Write",
			},
		},
	}, nil
}

// executePostEditCommands executes any matching post-edit commands for the given file path
func (t *FilesystemTool) executePostEditCommands(ctx context.Context, filePath string) error {
	if len(t.postEditCommands) == 0 {
		return nil
	}

	for _, postEdit := range t.postEditCommands {
		matched, err := filepath.Match(postEdit.Path, filepath.Base(filePath))
		if err != nil {
			slog.WarnContext(ctx, "Invalid post-edit pattern", "pattern", postEdit.Path, "error", err)
			continue
		}
		if !matched {
			continue
		}

		cmd := exec.CommandContext(ctx, "/bin/sh", "-c", postEdit.Cmd)
		cmd.Env = cmd.Environ()
		cmd.Env = append(cmd.Env, "path="+filePath)

		if err := cmd.Run(); err != nil {
			return fmt.Errorf("post-edit command failed for %s: %w", filePath, err)
		}

	}
	return nil
}

// Security helper to check if path is allowed
func (t *FilesystemTool) isPathAllowed(path string) error {
	absPath, err := filepath.Abs(path)
	if err != nil {
		return fmt.Errorf("unable to resolve absolute path: %w", err)
	}

	if len(t.allowedDirectories) == 0 {
		return fmt.Errorf("no allowed directories configured")
	}

	for _, allowedDir := range t.allowedDirectories {
		allowedAbs, err := filepath.Abs(allowedDir)
		if err != nil {
			continue
		}

		// Check if path matches exactly or is a subdirectory (with separator check)
		// This prevents sibling directories with similar names from bypassing the check
		// (e.g., /project-secrets when /project is allowed)
		if absPath == allowedAbs || strings.HasPrefix(absPath, allowedAbs+string(filepath.Separator)) {
			return nil
		}
	}

	return fmt.Errorf("path %s is not within allowed directories", path)
}

// initGitignoreMatchers initializes gitignore matchers for each allowed directory
func (t *FilesystemTool) initGitignoreMatchers() {
	for _, allowedDir := range t.allowedDirectories {
		absDir, err := filepath.Abs(allowedDir)
		if err != nil {
			slog.Warn("Failed to get absolute path for allowed directory", "dir", allowedDir, "error", err)
			continue
		}
		t.loadMatcherForDirectory(absDir)
	}
}

// loadMatcherForDirectory loads a gitignore matcher for a specific directory
func (t *FilesystemTool) loadMatcherForDirectory(absDir string) {
	// Use the shared VCS matcher from fsx package
	matcher, err := fsx.NewVCSMatcher(absDir)
	if err != nil {
		slog.Warn("Failed to create VCS matcher", "path", absDir, "error", err)
		return
	}

	// If no matcher (not in a git repo), just return
	if matcher == nil {
		return
	}

	// Skip if we already have a matcher for this repository
	if _, exists := t.repoMatchers[matcher.RepoRoot()]; exists {
		return
	}

	// Store the matcher
	t.repoMatchers[matcher.RepoRoot()] = matcher
}

// shouldIgnorePath checks if a path should be ignored based on VCS rules
func (t *FilesystemTool) shouldIgnorePath(path string) bool {
	if !t.ignoreVCS {
		return false
	}

	// Always ignore .git directories and their contents
	normalizedPath := filepath.ToSlash(path)
	if strings.Contains(normalizedPath, "/.git/") || strings.HasSuffix(normalizedPath, "/.git") {
		return true
	}

	// Check gitignore patterns using VCS matchers
	for _, matcher := range t.repoMatchers {
		// Delegate to the VCSMatcher's ShouldIgnore method
		if matcher.ShouldIgnore(path) {
			return true
		}
	}

	return false
}

// Handler implementations

func (t *FilesystemTool) handleDirectoryTree(_ context.Context, args DirectoryTreeArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	tree, err := fsx.DirectoryTree(args.Path, t.isPathAllowed, t.shouldIgnorePath, maxFiles)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error building directory tree: %s", err)), nil
	}

	result, err := json.MarshalIndent(tree, "", "  ")
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error formatting tree: %s", err)), nil
	}

	return tools.ResultSuccess(string(result)), nil
}

func (t *FilesystemTool) handleEditFile(ctx context.Context, args EditFileArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	content, err := os.ReadFile(args.Path)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error reading file: %s", err)), nil
	}

	originalContent := string(content)
	modifiedContent := originalContent

	var changes []string
	for i, edit := range args.Edits {
		if !strings.Contains(modifiedContent, edit.OldText) {
			return tools.ResultError(fmt.Sprintf("Edit %d failed: old text not found", i+1)), nil
		}
		modifiedContent = strings.Replace(modifiedContent, edit.OldText, edit.NewText, 1)
		changes = append(changes, fmt.Sprintf("Edit %d: Replaced %d characters", i+1, len(edit.OldText)))
	}

	if err := os.WriteFile(args.Path, []byte(modifiedContent), 0o644); err != nil {
		return tools.ResultError(fmt.Sprintf("Error writing file: %s", err)), nil
	}

	// Execute post-edit commands
	if err := t.executePostEditCommands(ctx, args.Path); err != nil {
		return tools.ResultError(fmt.Sprintf("File edited successfully but post-edit command failed: %s", err)), nil
	}

	if len(changes) == 1 {
		return tools.ResultSuccess(fmt.Sprintf("File edited successfully. %s", strings.TrimPrefix(changes[0], "Edit 1: "))), nil
	}

	return tools.ResultSuccess(fmt.Sprintf("File edited successfully. Changes:\n%s", strings.Join(changes, "\n"))), nil
}

func (t *FilesystemTool) handleListAllowedDirectories(_ context.Context, _ map[string]any) (*tools.ToolCallResult, error) {
	result, err := json.Marshal(t.allowedDirectories)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error formatting directories: %s", err)), nil
	}

	return &tools.ToolCallResult{
		Output: string(result),
		Meta:   t.allowedDirectories,
	}, nil
}

func (t *FilesystemTool) handleAddAllowedDirectory(_ context.Context, args AddAllowedDirectoryArgs) (*tools.ToolCallResult, error) {
	// Validate the path exists and is a directory
	absPath, err := filepath.Abs(args.Path)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error resolving path: %s", err)), nil
	}

	info, err := os.Stat(absPath)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error accessing path: %s", err)), nil
	}

	if !info.IsDir() {
		return tools.ResultError(fmt.Sprintf("Error: %s is not a directory", absPath)), nil
	}

	// Check if the directory is already allowed
	for _, allowedDir := range t.allowedDirectories {
		allowedAbs, err := filepath.Abs(allowedDir)
		if err != nil {
			continue
		}
		if allowedAbs == absPath {
			return tools.ResultError(fmt.Sprintf("Directory %s is already in allowed directories list", absPath)), nil
		}
		// Check if the requested path is already covered by an existing allowed directory
		// Use proper separator check to prevent sibling directory bypass
		if absPath == allowedAbs || strings.HasPrefix(absPath, allowedAbs+string(filepath.Separator)) {
			return tools.ResultSuccess(fmt.Sprintf("Directory %s is already accessible (covered by %s)", absPath, allowedAbs)), nil
		}
	}

	// User has confirmed, add the directory
	return t.addAllowedDirectory(absPath)
}

// addAllowedDirectory adds a directory to the allowed directories list
func (t *FilesystemTool) addAllowedDirectory(absPath string) (*tools.ToolCallResult, error) {
	t.allowedDirectories = append(t.allowedDirectories, absPath)

	// Load gitignore matcher for the new directory if VCS ignoring is enabled
	if t.ignoreVCS {
		t.loadMatcherForDirectory(absPath)
	}

	successMsg := fmt.Sprintf(`Directory successfully added to allowed directories list.

Added: %s

The agent now has filesystem access to this directory and all its subdirectories.

Updated allowed directories:
%s`, absPath, strings.Join(t.allowedDirectories, "\n"))

	return tools.ResultSuccess(successMsg), nil
}

func (t *FilesystemTool) handleListDirectory(_ context.Context, args ListDirectoryArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	entries, err := os.ReadDir(args.Path)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error reading directory: %s", err)), nil
	}

	var result strings.Builder
	meta := ListDirectoryMeta{}
	count := 0
	for _, entry := range entries {
		entryPath := filepath.Join(args.Path, entry.Name())
		if t.shouldIgnorePath(entryPath) {
			continue
		}

		if entry.IsDir() {
			fmt.Fprintf(&result, "DIR  %s\n", entry.Name())
			meta.Dirs = append(meta.Dirs, entry.Name())
		} else {
			fmt.Fprintf(&result, "FILE %s\n", entry.Name())
			meta.Files = append(meta.Files, entry.Name())
		}
		count++
		if count >= maxFiles {
			result.WriteString("...output truncated due to file limit...\n")
			meta.Truncated = true
			break
		}
	}

	return &tools.ToolCallResult{
		Output: result.String(),
		Meta:   meta,
	}, nil
}

func (t *FilesystemTool) handleReadFile(_ context.Context, args ReadFileArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	content, err := os.ReadFile(args.Path)
	if err != nil {
		var errMsg string
		if os.IsNotExist(err) {
			errMsg = "not found"
		} else {
			errMsg = err.Error()
		}

		return &tools.ToolCallResult{
			Output:  errMsg,
			IsError: true,
			Meta: ReadFileMeta{
				Path:  args.Path,
				Error: errMsg,
			},
		}, nil
	}

	return &tools.ToolCallResult{
		Output: string(content),
		Meta: ReadFileMeta{
			Path:      args.Path,
			Content:   string(content),
			LineCount: strings.Count(string(content), "\n") + 1,
		},
	}, nil
}

func (t *FilesystemTool) handleReadMultipleFiles(ctx context.Context, args ReadMultipleFilesArgs) (*tools.ToolCallResult, error) {
	type PathContent struct {
		Path    string `json:"path"`
		Content string `json:"content"`
	}

	var contents []PathContent
	var meta ReadMultipleFilesMeta

	for _, path := range args.Paths {
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}

		entry := ReadFileMeta{Path: path}

		if err := t.isPathAllowed(path); err != nil {
			contents = append(contents, PathContent{
				Path:    path,
				Content: err.Error(),
			})
			entry.Error = err.Error()
			meta.Files = append(meta.Files, entry)
			continue
		}

		content, err := os.ReadFile(path)
		if err != nil {
			errMsg := err.Error()
			if os.IsNotExist(err) {
				errMsg = "not found"
			}
			contents = append(contents, PathContent{
				Path:    path,
				Content: errMsg,
			})
			entry.Error = errMsg
			meta.Files = append(meta.Files, entry)
			continue
		}

		contents = append(contents, PathContent{
			Path:    path,
			Content: string(content),
		})
		entry.Content = string(content)
		entry.LineCount = strings.Count(string(content), "\n") + 1
		meta.Files = append(meta.Files, entry)
	}

	var output string
	if args.JSON {
		jsonResult, err := json.MarshalIndent(contents, "", "  ")
		if err != nil {
			return tools.ResultError(fmt.Sprintf("Error formatting JSON: %s", err)), nil
		}
		output = string(jsonResult)
	} else {
		var result strings.Builder
		for _, content := range contents {
			fmt.Fprintf(&result, "=== %s ===\n%s\n\n", content.Path, content.Content)
		}
		output = result.String()
	}

	return &tools.ToolCallResult{
		Output: output,
		Meta:   meta,
	}, nil
}

func (t *FilesystemTool) handleSearchFiles(_ context.Context, args SearchFilesArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	var matches []string

	err := filepath.WalkDir(args.Path, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil // Skip errors and continue
		}

		if err := t.isPathAllowed(path); err != nil {
			return nil // Skip disallowed paths
		}

		// Check VCS ignore rules
		if t.shouldIgnorePath(path) {
			if d.IsDir() {
				return fs.SkipDir
			}
			return nil
		}

		// Check exclude patterns against relative path from search root
		relPath, err := filepath.Rel(args.Path, path)
		if err != nil {
			return nil
		}

		for _, exclude := range args.ExcludePatterns {
			if matchExcludePattern(exclude, relPath) {
				if d.IsDir() {
					return fs.SkipDir
				}
				return nil
			}
		}
		if match(args.Pattern, filepath.Base(path)) && !d.IsDir() {
			matches = append(matches, path)
		}

		return nil
	})
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error searching files: %s", err)), nil
	}

	if len(matches) == 0 {
		return tools.ResultSuccess("No files found"), nil
	}

	return tools.ResultSuccess(fmt.Sprintf("%d files found:\n%s", len(matches), strings.Join(matches, "\n"))), nil
}

func (t *FilesystemTool) handleSearchFilesContent(_ context.Context, args SearchFilesContentArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	var regex *regexp.Regexp
	if args.IsRegex {
		var err error
		regex, err = regexp.Compile(args.Query)
		if err != nil {
			return tools.ResultError(fmt.Sprintf("Invalid regex pattern: %s", err)), nil
		}
	}

	var results []string

	err := filepath.WalkDir(args.Path, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}

		if err := t.isPathAllowed(path); err != nil {
			return nil
		}

		// Check VCS ignore rules
		if t.shouldIgnorePath(path) {
			if d.IsDir() {
				return fs.SkipDir
			}
			return nil
		}

		// Check exclude patterns against relative path from search root
		relPath, err := filepath.Rel(args.Path, path)
		if err != nil {
			return nil
		}

		for _, exclude := range args.ExcludePatterns {
			if matchExcludePattern(exclude, relPath) {
				if d.IsDir() {
					return fs.SkipDir // Skip entire directory
				}
				return nil // Skip this file
			}
		}

		// Only process files, not directories
		if d.IsDir() {
			return nil
		}

		content, err := os.ReadFile(path)
		if err != nil {
			return nil
		}

		lines := strings.Split(string(content), "\n")
		for lineNum, line := range lines {
			var matched bool
			var matchStart, matchEnd int

			if args.IsRegex {
				if loc := regex.FindStringIndex(line); loc != nil {
					matched = true
					matchStart, matchEnd = loc[0], loc[1]
				}
			} else {
				if idx := strings.Index(line, args.Query); idx != -1 {
					matched = true
					matchStart, matchEnd = idx, idx+len(args.Query)
				}
			}

			if matched {
				preview := line
				if len(preview) > 100 {
					start := max(matchStart-20, 0)
					end := matchEnd + 20
					end = min(end, len(preview))
					preview = preview[start:end]
				}

				result := fmt.Sprintf("%s:%d:%d: %s", path, lineNum+1, matchStart+1, preview)
				results = append(results, result)
			}
		}

		return nil
	})
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error searching file contents: %s", err)), nil
	}

	if len(results) == 0 {
		return tools.ResultSuccess("No results found"), nil
	}

	return tools.ResultSuccess(strings.Join(results, "\n")), nil
}

func (t *FilesystemTool) handleWriteFile(ctx context.Context, args WriteFileArgs) (*tools.ToolCallResult, error) {
	if err := t.isPathAllowed(args.Path); err != nil {
		return tools.ResultError(err.Error()), nil
	}

	// Create parent directory structure if it doesn't exist
	dir := filepath.Dir(args.Path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return tools.ResultError(fmt.Sprintf("Error creating directory structure: %s", err)), nil
	}

	if err := os.WriteFile(args.Path, []byte(args.Content), 0o644); err != nil {
		return tools.ResultError(fmt.Sprintf("Error writing file: %s", err)), nil
	}

	// Execute post-edit commands
	if err := t.executePostEditCommands(ctx, args.Path); err != nil {
		return tools.ResultError(fmt.Sprintf("File written successfully but post-edit command failed: %s", err)), nil
	}

	return tools.ResultSuccess(fmt.Sprintf("File written successfully: %s (%d bytes)", args.Path, len(args.Content))), nil
}

// matchExcludePattern checks if a path should be excluded based on the exclude pattern
// It supports glob patterns and directory wildcards like .git/*
func matchExcludePattern(pattern, relPath string) bool {
	// Normalize path separators to forward slashes for consistent matching
	normalizedPath := filepath.ToSlash(relPath)
	normalizedPattern := filepath.ToSlash(pattern)

	// Handle directory patterns ending with /*
	if dirPattern, found := strings.CutSuffix(normalizedPattern, "/*"); found {
		// Check if path starts with the directory pattern
		if strings.HasPrefix(normalizedPath, dirPattern+"/") || normalizedPath == dirPattern {
			return true
		}
	}

	// Try glob pattern matching on the full relative path
	if matched, _ := filepath.Match(normalizedPattern, normalizedPath); matched {
		return true
	}

	// Try glob pattern matching on just the base name for backwards compatibility
	if matched, _ := filepath.Match(normalizedPattern, filepath.Base(normalizedPath)); matched {
		return true
	}

	// Check if pattern matches any parent directory path
	pathParts := strings.Split(normalizedPath, "/")
	for i := range pathParts {
		subPath := strings.Join(pathParts[:i+1], "/")
		if matched, _ := filepath.Match(normalizedPattern, subPath); matched {
			return true
		}
	}

	return false
}

func match(pattern, name string) bool {
	matched, _ := doublestar.Match(pattern, name)
	return matched || strings.Contains(strings.ToLower(name), strings.ToLower(pattern))
}
