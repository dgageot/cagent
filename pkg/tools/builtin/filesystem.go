package builtin

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"log/slog"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"sync"

	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/trace"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/fsx"
	"github.com/docker/docker-agent/pkg/shellpath"
	"github.com/docker/docker-agent/pkg/tools"
)

// annotateFilesystemSpan stamps the operation kind and target path
// onto the active runtime.tool.handler span. Paths ship unconditionally
// — they're the main signal of what the agent touched. Drop or hash
// `cagent.tool.filesystem.path` at the OTel collector if paths
// routinely reveal identifiers you don't want shipped.
func annotateFilesystemSpan(ctx context.Context, op, path string) {
	span := trace.SpanFromContext(ctx)
	if !span.IsRecording() {
		return
	}
	attrs := []attribute.KeyValue{
		attribute.String("cagent.tool.filesystem.op", op),
	}
	if path != "" {
		attrs = append(attrs, attribute.String("cagent.tool.filesystem.path", path))
	}
	span.SetAttributes(attrs...)
}

const (
	ToolNameReadFile           = "read_file"
	ToolNameReadMultipleFiles  = "read_multiple_files"
	ToolNameEditFile           = "edit_file"
	ToolNameWriteFile          = "write_file"
	ToolNameDirectoryTree      = "directory_tree"
	ToolNameListDirectory      = "list_directory"
	ToolNameSearchFilesContent = "search_files_content"
	ToolNameMkdir              = "create_directory"
	ToolNameRmdir              = "remove_directory"
)

// PostEditConfig represents a post-edit command configuration
type PostEditConfig struct {
	Path string // File path pattern (glob-style)
	Cmd  string // Command to execute (with $path placeholder)
}

type FilesystemTool struct {
	workingDir       string
	postEditCommands []PostEditConfig
	ignoreVCS        bool
	repoMatcher      *fsx.VCSMatcher
	repoMatcherOnce  sync.Once

	// sandboxBroken is set when allow/deny list construction fails.
	// When true, all filesystem operations are rejected (fail-closed).
	sandboxBroken bool

	// allowList, when non-nil, restricts every filesystem operation to paths
	// that resolve under one of the listed roots. nil means "no allow-list";
	// the toolset accepts any path the OS will let it touch.
	allowList *pathRootSet
	// denyList, when non-nil, rejects every filesystem operation on paths
	// that resolve under one of the listed roots, even when the path also
	// matches the allow-list. nil means "no deny-list".
	denyList *pathRootSet
}

// Verify interface compliance
var (
	_ tools.ToolSet      = (*FilesystemTool)(nil)
	_ tools.Instructable = (*FilesystemTool)(nil)
	_ io.Closer          = (*FilesystemTool)(nil)
)

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

// WithAllowList restricts every filesystem operation to paths that resolve
// under one of the supplied roots. Each entry may be:
//   - "." — the agent's working directory
//   - "~" or "~/..." — the user's home directory
//   - "$VAR" / "${VAR}" — an environment variable
//   - any absolute or relative path (relative paths are anchored at the
//     working directory)
//
// Symlinks are resolved before the containment check, so a symlink inside an
// allowed root cannot be used to escape it. An empty or nil slice disables
// the allow-list and preserves the default behaviour (any path is allowed).
//
// Invalid entries (e.g. an empty string) are logged and the allow-list is
// silently dropped, mirroring how WithIgnoreVCS handles construction errors.
func WithAllowList(roots []string) FileSystemOpt {
	return func(t *FilesystemTool) {
		set, err := newPathRootSet(t.workingDir, roots)
		if err != nil {
			slog.Error("filesystem allow-list: invalid entry; disabling toolset", "error", err)
			t.sandboxBroken = true
			return
		}
		t.allowList = set
	}
}

// WithDenyList forbids every filesystem operation on paths that resolve under
// one of the supplied roots. Tokens follow the same expansion rules as
// [WithAllowList]. The deny-list takes precedence over the allow-list: a
// path that matches both is rejected. An empty or nil slice disables the
// deny-list.
func WithDenyList(roots []string) FileSystemOpt {
	return func(t *FilesystemTool) {
		set, err := newPathRootSet(t.workingDir, roots)
		if err != nil {
			slog.Error("filesystem deny-list: invalid entry; disabling toolset", "error", err)
			t.sandboxBroken = true
			return
		}
		t.denyList = set
	}
}

func NewFilesystemTool(workingDir string, opts ...FileSystemOpt) *FilesystemTool {
	t := &FilesystemTool{
		workingDir: workingDir,
	}

	for _, opt := range opts {
		opt(t)
	}

	return t
}

// Close releases any *os.Root file descriptors held by the allow/deny lists.
// It is safe to call Close multiple times.
func (t *FilesystemTool) Close() error {
	if t.allowList != nil {
		t.allowList.close()
	}
	if t.denyList != nil {
		t.denyList.close()
	}
	return nil
}

func (t *FilesystemTool) Instructions() string {
	var b strings.Builder
	b.WriteString(`## Filesystem Tools

- Relative paths resolve from the working directory; absolute paths and ".." work as expected
- Prefer read_multiple_files over sequential read_file calls
- Use search_files_content to locate code or text across files
- Use exclude patterns in searches and max_depth in directory_tree to limit output`)
	if d := t.allowList.describe(); d != "" {
		fmt.Fprintf(&b, "\n- These tools are restricted to paths under: %s. Any other path is rejected without touching the filesystem.", d)
	}
	if d := t.denyList.describe(); d != "" {
		fmt.Fprintf(&b, "\n- These tools must not access paths under: %s. Such paths are rejected without touching the filesystem.", d)
	}
	return b.String()
}

type DirectoryTreeArgs struct {
	Path string `json:"path" jsonschema:"The directory path to traverse (relative to working directory)"`
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

type SearchFilesContentArgs struct {
	Path            string   `json:"path" jsonschema:"The starting directory path"`
	Query           string   `json:"query" jsonschema:"The text or regex pattern to search for"`
	IsRegex         bool     `json:"is_regex,omitempty" jsonschema:"If true, treat query as regex; otherwise literal text"`
	ExcludePatterns []string `json:"excludePatterns,omitempty" jsonschema:"Patterns to exclude from search"`
}

type SearchFilesContentMeta struct {
	MatchCount int `json:"matchCount"`
	FileCount  int `json:"fileCount"`
}

type ListDirectoryArgs struct {
	Path string `json:"path" jsonschema:"The directory path to list"`
}

type CreateDirectoryArgs struct {
	Paths []string `json:"paths" jsonschema:"Array of directory paths to create"`
}

type RemoveDirectoryArgs struct {
	Paths []string `json:"paths" jsonschema:"Array of directory paths to remove"`
}

type ListDirectoryMeta struct {
	Files     []string `json:"files"`
	Dirs      []string `json:"dirs"`
	Truncated bool     `json:"truncated"`
}

type DirectoryTreeMeta struct {
	FileCount int  `json:"fileCount"`
	DirCount  int  `json:"dirCount"`
	Truncated bool `json:"truncated"`
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

// ParseEditFileArgs parses LLM-generated edit_file arguments, handling two
// common failure modes:
//  1. The outer JSON itself is malformed — typically extra closing braces/brackets
//     or stray escape sequences caused by the model losing track of nesting depth
//     when the text payload contains structural characters (e.g. YAML, Dockerfiles).
//  2. The "edits" field is double-serialized (a JSON string instead of an array).
func ParseEditFileArgs(data []byte) (EditFileArgs, error) {
	var raw struct {
		Path  string          `json:"path"`
		Edits json.RawMessage `json:"edits"`
	}

	if err := json.Unmarshal(data, &raw); err != nil {
		repaired, ok := tryRepairEditFileJSON(data)
		if !ok {
			return EditFileArgs{}, fmt.Errorf("failed to parse edit_file arguments: %w", err)
		}
		if err := json.Unmarshal(repaired, &raw); err != nil {
			return EditFileArgs{}, fmt.Errorf("failed to parse edit_file arguments after repair: %w", err)
		}
		slog.Debug("Repaired malformed edit_file JSON arguments")
	}

	args := EditFileArgs{Path: raw.Path}

	// When edits is missing or null (e.g. during argument streaming in
	// the TUI, or partial tool calls), accept the partial result.
	if len(raw.Edits) == 0 || string(raw.Edits) == "null" {
		return args, nil
	}

	// Try parsing edits as an array first (normal case).
	if err := json.Unmarshal(raw.Edits, &args.Edits); err == nil {
		return args, nil
	}

	// Try unwrapping a double-serialized JSON string.
	var editsStr string
	if err := json.Unmarshal(raw.Edits, &editsStr); err != nil {
		return EditFileArgs{}, fmt.Errorf("edits field is neither an array nor a JSON string: %w", err)
	}
	if err := json.Unmarshal([]byte(editsStr), &args.Edits); err != nil {
		return EditFileArgs{}, fmt.Errorf("failed to parse double-serialized edits string: %w", err)
	}

	return args, nil
}

// tryRepairEditFileJSON attempts to fix common LLM JSON malformations by
// iteratively removing the offending character(s) at each json.SyntaxError
// offset. Observed failure modes from production sessions:
//
//   - Extra '}' — model loses brace count (e.g. "}}]}" instead of "}]}")
//   - Extra ']' — model adds a spurious array wrapper
//   - Stray '\' — model emits an escape sequence outside of a string value
//     (e.g. literal \n between tokens, or \" where " is expected)
func tryRepairEditFileJSON(data []byte) ([]byte, bool) {
	current := append([]byte(nil), data...) // defensive copy
	for range 3 {
		var synErr *json.SyntaxError
		if err := json.Unmarshal(current, &json.RawMessage{}); err == nil {
			return current, true
		} else if !errors.As(err, &synErr) {
			return nil, false
		}

		// json.SyntaxError.Offset is 1-based.
		offset := int(synErr.Offset) - 1
		if offset < 0 || offset >= len(current) {
			return nil, false
		}

		ch := current[offset]
		removeCount := 1

		switch ch {
		case '}', ']':
			// Extra closing delimiter — just remove it.
		case '\\':
			// Stray escape sequence outside a string value. For \n, \t, \r
			// both characters are garbage so remove them. For \" the quote
			// is a valid structural character (string delimiter), so only
			// strip the backslash.
			if offset+1 < len(current) {
				switch current[offset+1] {
				case 'n', 't', 'r':
					removeCount = 2
				}
			}
		default:
			return nil, false
		}

		repaired := make([]byte, 0, len(current)-removeCount)
		repaired = append(repaired, current[:offset]...)
		repaired = append(repaired, current[offset+removeCount:]...)
		current = repaired
	}

	if json.Valid(current) {
		return current, true
	}
	return nil, false
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
			Handler: tools.NewHandler(t.handleDirectoryTree),
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
			Handler:      t.editFileHandler(),
			Annotations: tools.ToolAnnotations{
				Title: "Edit",
			},
			AddDescriptionParameter: true,
		},
		{
			Name:         ToolNameListDirectory,
			Category:     "filesystem",
			Description:  "Get a detailed listing of all files and directories in a specified path.",
			Parameters:   tools.MustSchemaFor[ListDirectoryArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleListDirectory),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "List Directory",
			},
			AddDescriptionParameter: true,
		},
		{
			Name:         ToolNameReadFile,
			Category:     "filesystem",
			Description:  "Read the complete contents of a file from the file system. Supports text files and images (jpg, png, gif, webp). Images are returned as image content that you can view directly.",
			Parameters:   tools.MustSchemaFor[ReadFileArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleReadFile),
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
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleReadMultipleFiles),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Read Multiple Files",
			},
		},
		{
			Name:         ToolNameSearchFilesContent,
			Category:     "filesystem",
			Description:  "Searches for text or regex patterns in the content of files matching a GLOB pattern.",
			Parameters:   tools.MustSchemaFor[SearchFilesContentArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleSearchFilesContent),
			Annotations: tools.ToolAnnotations{
				ReadOnlyHint: true,
				Title:        "Search Files Content",
			},
			AddDescriptionParameter: true,
		},
		{
			Name:         ToolNameWriteFile,
			Category:     "filesystem",
			Description:  "Create a new file or completely overwrite an existing file with new content.",
			Parameters:   tools.MustSchemaFor[WriteFileArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleWriteFile),
			Annotations: tools.ToolAnnotations{
				Title: "Write",
			},
			AddDescriptionParameter: true,
		},
		{
			Name:         ToolNameMkdir,
			Category:     "filesystem",
			Description:  "Create one or more new directories or nested directory structures.",
			Parameters:   tools.MustSchemaFor[CreateDirectoryArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleCreateDirectory),
			Annotations: tools.ToolAnnotations{
				Title: "Create Directory",
			},
		},
		{
			Name:         ToolNameRmdir,
			Category:     "filesystem",
			Description:  "Remove one or more empty directories.",
			Parameters:   tools.MustSchemaFor[RemoveDirectoryArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      tools.NewHandler(t.handleRemoveDirectory),
			Annotations: tools.ToolAnnotations{
				Title: "Remove Directory",
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

		shell, argsPrefix := shellpath.DetectShell()
		cmd := exec.CommandContext(ctx, shell, append(argsPrefix, postEdit.Cmd)...)
		cmd.Env = cmd.Environ()
		cmd.Env = append(cmd.Env, "file="+filePath)

		if err := cmd.Run(); err != nil {
			return fmt.Errorf("post-edit command failed for %s: %w", filePath, err)
		}
	}
	return nil
}

// resolvePath resolves a path relative to the working directory.
// Relative paths (including ".") are joined with the working directory.
// Absolute paths and paths starting with ".." are used as-is.
//
// resolvePath does NOT enforce the allow- or deny-lists; callers should use
// [resolveAndCheckPath] when those checks are required (i.e. for any path
// that originates from a tool argument).
func (t *FilesystemTool) resolvePath(path string) string {
	if filepath.IsAbs(path) {
		return filepath.Clean(path)
	}

	return filepath.Clean(filepath.Join(t.workingDir, path))
}

// resolveAndCheckPath is the canonical entry point used by every filesystem
// handler that operates on a user-supplied path. It resolves the path against
// the working directory and validates the result against the allow- and
// deny-lists. The returned string is the on-disk path the handler should pass
// to the os.* call.
//
// When neither list is configured the function is functionally equivalent to
// [resolvePath]. When a list is configured, symlinks are resolved before the
// containment check so that a symlink inside an allowed directory cannot leak
// access outside it. Paths that don't exist yet (e.g. for write_file or
// create_directory) are checked against their nearest existing ancestor so
// the caller can still create them.
//
// The deny-list takes precedence over the allow-list: a path that matches
// both is rejected.
//
// SECURITY NOTE: there is a small TOCTOU window between this check and the
// subsequent os.* call. A concurrent process running as the same user could
// in principle replace a directory with a symlink between the two and cause
// the I/O to escape the allowed area. This is acceptable because:
//
//   - The intended threat model is the LLM itself — not another local
//     process. The LLM exercises the toolset only through the exposed
//     handlers (read/write/list/etc.) and has no symlink-creation primitive,
//     so it cannot win this race from inside the agent.
//   - The toolset still defends against the static cases that matter: pre-
//     existing symlinks, ".." traversals, absolute paths outside the allow-
//     list. Those are checked here AND verified through [*os.Root].Lstat
//     when an [*os.Root] is available.
//
// Callers wanting full TOCTOU safety should perform their I/O through the
// [*os.Root] handles owned by [pathRootSet] rather than via os.* on the
// returned path.
func (t *FilesystemTool) resolveAndCheckPath(path string) (string, error) {
	if t.sandboxBroken {
		return "", errors.New("filesystem toolset is disabled due to invalid allow/deny list configuration")
	}

	resolved := t.resolvePath(path)
	if t.allowList == nil && t.denyList == nil {
		return resolved, nil
	}
	realPath, err := resolveRealPath(resolved)
	if err != nil {
		return "", fmt.Errorf("resolving %q: %w", path, err)
	}
	if t.denyList != nil && t.denyList.contains(realPath) {
		return "", fmt.Errorf("path %q is inside a denied directory (%s)", path, t.denyList.describe())
	}
	if t.allowList != nil && !t.allowList.contains(realPath) {
		return "", fmt.Errorf("path %q is outside the allowed directories (%s)", path, t.allowList.describe())
	}
	return resolved, nil
}

// initGitignoreMatcher initializes the gitignore matcher for the working directory.
// It is safe to call multiple times; initialization only happens once.
func (t *FilesystemTool) initGitignoreMatcher() {
	if !t.ignoreVCS {
		return
	}

	t.repoMatcherOnce.Do(func() {
		absDir, err := filepath.Abs(t.workingDir)
		if err != nil {
			slog.Warn("Failed to get absolute path for working directory", "dir", t.workingDir, "error", err)
			return
		}

		matcher, err := fsx.NewVCSMatcher(absDir)
		if err != nil {
			slog.Warn("Failed to create VCS matcher", "path", absDir, "error", err)
			return
		}

		t.repoMatcher = matcher
	})
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

	// Lazily initialize the gitignore matcher on first use
	t.initGitignoreMatcher()

	return t.repoMatcher != nil && t.repoMatcher.ShouldIgnore(path)
}

// Handler implementations

func (t *FilesystemTool) handleDirectoryTree(ctx context.Context, args DirectoryTreeArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "directory_tree", args.Path)
	resolvedPath, err := t.resolveAndCheckPath(args.Path)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	// Create a path checker that enforces allow/deny lists for every child path.
	// This prevents symlinked directories from being traversed outside the sandbox.
	pathChecker := func(childPath string) error {
		_, err := t.resolveAndCheckPath(childPath)
		return err
	}

	tree, err := fsx.DirectoryTree(ctx, resolvedPath, pathChecker, t.shouldIgnorePath, maxFiles)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error building directory tree: %s", err)), nil
	}

	result, err := json.Marshal(tree)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error formatting tree: %s", err)), nil
	}

	fileCount, dirCount := countTreeNodes(tree)
	meta := DirectoryTreeMeta{
		FileCount: fileCount,
		DirCount:  dirCount,
		Truncated: fileCount+dirCount >= maxFiles,
	}

	return &tools.ToolCallResult{
		Output: string(result),
		Meta:   meta,
	}, nil
}

func countTreeNodes(node *fsx.TreeNode) (files, dirs int) {
	if node == nil {
		return 0, 0
	}
	if node.Type == "file" {
		return 1, 0
	}
	if node.Type == "directory" {
		dirs = 1
		for _, child := range node.Children {
			f, d := countTreeNodes(child)
			files += f
			dirs += d
		}
	}
	return files, dirs
}

// editFileHandler returns a ToolHandler that parses edit_file arguments with
// repair logic for malformed JSON, then delegates to handleEditFile.
// This bypasses tools.NewHandler because Go's json.Unmarshal scanner rejects
// structurally invalid JSON before calling any custom UnmarshalJSON method.
func (t *FilesystemTool) editFileHandler() tools.ToolHandler {
	return func(ctx context.Context, toolCall tools.ToolCall) (*tools.ToolCallResult, error) {
		data := toolCall.Function.Arguments
		if data == "" {
			data = "{}"
		}
		args, err := ParseEditFileArgs([]byte(data))
		if err != nil {
			return nil, err
		}
		return t.handleEditFile(ctx, args)
	}
}

func (t *FilesystemTool) handleEditFile(ctx context.Context, args EditFileArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "edit_file", args.Path)
	resolvedPath, err := t.resolveAndCheckPath(args.Path)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	content, err := os.ReadFile(resolvedPath)
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

	if err := os.WriteFile(resolvedPath, []byte(modifiedContent), 0o644); err != nil {
		return tools.ResultError(fmt.Sprintf("Error writing file: %s", err)), nil
	}

	if err := t.executePostEditCommands(ctx, resolvedPath); err != nil {
		return tools.ResultError(fmt.Sprintf("File edited successfully but post-edit command failed: %s", err)), nil
	}

	if len(changes) == 1 {
		return tools.ResultSuccess("File edited successfully. " + strings.TrimPrefix(changes[0], "Edit 1: ")), nil
	}

	return tools.ResultSuccess("File edited successfully. Changes:\n" + strings.Join(changes, "\n")), nil
}

func (t *FilesystemTool) handleListDirectory(ctx context.Context, args ListDirectoryArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "list_directory", args.Path)
	resolvedPath, err := t.resolveAndCheckPath(args.Path)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	entries, err := os.ReadDir(resolvedPath)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error reading directory: %s", err)), nil
	}

	var result strings.Builder
	meta := ListDirectoryMeta{}
	count := 0
	for _, entry := range entries {
		entryPath := filepath.Join(resolvedPath, entry.Name())
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

func (t *FilesystemTool) handleReadFile(ctx context.Context, args ReadFileArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "read_file", args.Path)
	resolvedPath, err := t.resolveAndCheckPath(args.Path)
	if err != nil {
		return &tools.ToolCallResult{
			Output:  err.Error(),
			IsError: true,
			Meta:    ReadFileMeta{Path: args.Path, Error: err.Error()},
		}, nil
	}

	// Check if the file exists before any type detection.
	info, err := os.Stat(resolvedPath)
	if err != nil {
		var errMsg string
		if errors.Is(err, fs.ErrNotExist) {
			errMsg = "not found"
		} else {
			errMsg = err.Error()
		}

		return &tools.ToolCallResult{
			Output:  errMsg,
			IsError: true,
			Meta: ReadFileMeta{
				Error: errMsg,
			},
		}, nil
	}

	// Only check for image files on regular files (not directories, etc.)
	if info.Mode().IsRegular() && chat.IsImageFile(resolvedPath) {
		return t.readImageFile(resolvedPath, args.Path)
	}

	content, err := os.ReadFile(resolvedPath)
	if err != nil {
		return &tools.ToolCallResult{
			Output:  err.Error(),
			IsError: true,
			Meta: ReadFileMeta{
				Error: err.Error(),
			},
		}, nil
	}

	text := string(content)

	return &tools.ToolCallResult{
		Output: text,
		Meta: ReadFileMeta{
			LineCount: strings.Count(text, "\n") + 1,
		},
	}, nil
}

// readImageFile reads an image file and returns it as base64-encoded image content.
// The caller must ensure the file exists (e.g. via os.Stat) before calling this method.
func (t *FilesystemTool) readImageFile(resolvedPath, originalPath string) (*tools.ToolCallResult, error) {
	data, err := os.ReadFile(resolvedPath)
	if err != nil {
		errMsg := err.Error()
		return &tools.ToolCallResult{
			Output:  errMsg,
			IsError: true,
			Meta: ReadFileMeta{
				Error: errMsg,
			},
		}, nil
	}

	mimeType := chat.DetectMimeType(resolvedPath)

	// Resize the image if it exceeds provider limits (max 2000×2000, max 4.5MB).
	resized, err := chat.ResizeImage(data, mimeType)
	if err != nil {
		// Check if the original exceeds limits before falling back
		if len(data) > chat.MaxImageBytes {
			return &tools.ToolCallResult{
				Output:  fmt.Sprintf("Error: Image file too large (%d bytes, max %d bytes)", len(data), chat.MaxImageBytes),
				IsError: true,
				Meta:    ReadFileMeta{Path: originalPath, Error: "image too large"},
			}, nil
		}
		// Original is within limits, proceed with fallback
		slog.Warn("Image resize failed, sending original (within limits)", "path", originalPath, "error", err)
		encoded := base64.StdEncoding.EncodeToString(data)
		return &tools.ToolCallResult{
			Output: fmt.Sprintf("Read image file %s [%s] (%d bytes)", originalPath, mimeType, len(data)),
			Images: []tools.ImageContent{{Data: encoded, MimeType: mimeType}},
			Meta:   ReadFileMeta{Path: originalPath},
		}, nil
	}

	encoded := base64.StdEncoding.EncodeToString(resized.Data)
	output := fmt.Sprintf("Read image file %s [%s] (%d bytes)", originalPath, resized.MimeType, len(resized.Data))
	if note := chat.FormatDimensionNote(resized); note != "" {
		output += "\n" + note
	}

	return &tools.ToolCallResult{
		Output: output,
		Images: []tools.ImageContent{{Data: encoded, MimeType: resized.MimeType}},
		Meta:   ReadFileMeta{Path: originalPath},
	}, nil
}

func (t *FilesystemTool) handleReadMultipleFiles(ctx context.Context, args ReadMultipleFilesArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "read_multiple_files", "")
	if span := trace.SpanFromContext(ctx); span.IsRecording() {
		span.SetAttributes(
			attribute.Int("cagent.tool.filesystem.path_count", len(args.Paths)),
			attribute.StringSlice("cagent.tool.filesystem.paths", args.Paths),
		)
	}
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

		resolvedPath, err := t.resolveAndCheckPath(path)
		if err != nil {
			errMsg := err.Error()
			contents = append(contents, PathContent{
				Path:    path,
				Content: errMsg,
			})
			entry.Error = errMsg
			meta.Files = append(meta.Files, entry)
			continue
		}

		content, err := os.ReadFile(resolvedPath)
		if err != nil {
			errMsg := err.Error()
			if errors.Is(err, fs.ErrNotExist) {
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

		text := string(content)
		contents = append(contents, PathContent{
			Path:    path,
			Content: text,
		})
		entry.Content = text
		entry.LineCount = strings.Count(text, "\n") + 1
		meta.Files = append(meta.Files, entry)
	}

	var output string
	if args.JSON {
		jsonResult, err := json.Marshal(contents)
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

func (t *FilesystemTool) handleSearchFilesContent(ctx context.Context, args SearchFilesContentArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "search_files_content", args.Path)
	resolvedPath, err := t.resolveAndCheckPath(args.Path)
	if err != nil {
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
	filesWithMatches := make(map[string]struct{})

	err = filepath.WalkDir(resolvedPath, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
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
		relPath, err := filepath.Rel(resolvedPath, path)
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

		// Only process files, not directories
		if d.IsDir() {
			return nil
		}

		// Check this file against allow/deny lists before reading.
		// This prevents symlinks inside the allowed root from escaping the sandbox.
		if _, checkErr := t.resolveAndCheckPath(path); checkErr != nil {
			// Skip files outside the sandbox silently (don't fail the whole search).
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
				filesWithMatches[path] = struct{}{}
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

	meta := SearchFilesContentMeta{
		MatchCount: len(results),
		FileCount:  len(filesWithMatches),
	}

	if len(results) == 0 {
		return &tools.ToolCallResult{
			Output: "No results found",
			Meta:   meta,
		}, nil
	}

	return &tools.ToolCallResult{
		Output: strings.Join(results, "\n"),
		Meta:   meta,
	}, nil
}

func (t *FilesystemTool) handleWriteFile(ctx context.Context, args WriteFileArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "write_file", args.Path)
	resolvedPath, err := t.resolveAndCheckPath(args.Path)
	if err != nil {
		return tools.ResultError(err.Error()), nil
	}

	// Create parent directory structure if it doesn't exist
	dir := filepath.Dir(resolvedPath)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return tools.ResultError(fmt.Sprintf("Error creating directory structure: %s", err)), nil
	}

	if err := os.WriteFile(resolvedPath, []byte(args.Content), 0o644); err != nil {
		return tools.ResultError(fmt.Sprintf("Error writing file: %s", err)), nil
	}

	if err := t.executePostEditCommands(ctx, resolvedPath); err != nil {
		return tools.ResultError(fmt.Sprintf("File written successfully but post-edit command failed: %s", err)), nil
	}

	return tools.ResultSuccess(fmt.Sprintf("File written successfully: %s (%d bytes)", args.Path, len(args.Content))), nil
}

func (t *FilesystemTool) handleCreateDirectory(ctx context.Context, args CreateDirectoryArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "create_directory", "")
	if span := trace.SpanFromContext(ctx); span.IsRecording() {
		span.SetAttributes(
			attribute.Int("cagent.tool.filesystem.path_count", len(args.Paths)),
			attribute.StringSlice("cagent.tool.filesystem.paths", args.Paths),
		)
	}
	var results []string
	for _, path := range args.Paths {
		resolvedPath, err := t.resolveAndCheckPath(path)
		if err != nil {
			return tools.ResultError(err.Error()), nil
		}
		if err := os.MkdirAll(resolvedPath, 0o755); err != nil {
			return tools.ResultError(fmt.Sprintf("Error creating directory %s: %s", path, err)), nil
		}
		results = append(results, "Directory created successfully: "+path)
	}

	return tools.ResultSuccess(strings.Join(results, "\n")), nil
}

func (t *FilesystemTool) handleRemoveDirectory(ctx context.Context, args RemoveDirectoryArgs) (*tools.ToolCallResult, error) {
	annotateFilesystemSpan(ctx, "remove_directory", "")
	if span := trace.SpanFromContext(ctx); span.IsRecording() {
		span.SetAttributes(
			attribute.Int("cagent.tool.filesystem.path_count", len(args.Paths)),
			attribute.StringSlice("cagent.tool.filesystem.paths", args.Paths),
		)
	}
	var results []string
	for _, path := range args.Paths {
		resolvedPath, err := t.resolveAndCheckPath(path)
		if err != nil {
			return tools.ResultError(err.Error()), nil
		}

		if err := rmdir(resolvedPath); err != nil {
			return tools.ResultError(fmt.Sprintf("Error removing directory %s: %s", path, err)), nil
		}
		results = append(results, "Directory removed successfully: "+path)
	}

	return tools.ResultSuccess(strings.Join(results, "\n")), nil
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
