package builtin

import (
	"bytes"
	"cmp"
	"context"
	"fmt"
	"os"
	"os/exec"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/docker/cagent/pkg/concurrent"
	"github.com/docker/cagent/pkg/config"
	"github.com/docker/cagent/pkg/tools"
)

const (
	ToolNameShell              = "shell"
	ToolNameRunShellBackground = "run_background_job"
	ToolNameListBackgroundJobs = "list_background_jobs"
	ToolNameViewBackgroundJob  = "view_background_job"
	ToolNameStopBackgroundJob  = "stop_background_job"
)

type ShellTool struct {
	tools.BaseToolSet
	handler *shellHandler
}

// Make sure Shell Tool implements the ToolSet Interface
var _ tools.ToolSet = (*ShellTool)(nil)

type shellHandler struct {
	shell           string
	shellArgsPrefix []string
	env             []string
	timeout         time.Duration
	workingDir      string
	jobs            *concurrent.Map[string, *backgroundJob]
	jobCounter      atomic.Int64
}

// Job status constants
const (
	statusRunning int32 = iota
	statusCompleted
	statusStopped
	statusFailed
)

// backgroundJob tracks a background shell command
type backgroundJob struct {
	id           string
	cmd          string
	cwd          string
	process      *os.Process
	processGroup *processGroup
	outputMu     sync.RWMutex
	output       *bytes.Buffer
	startTime    time.Time
	status       atomic.Int32
	exitCode     int
	err          error
}

// limitedWriter wraps a buffer and stops writing after maxSize bytes
type limitedWriter struct {
	mu      sync.Mutex
	buf     *bytes.Buffer
	written int64
	maxSize int64
}

func (lw *limitedWriter) Write(p []byte) (n int, err error) {
	lw.mu.Lock()
	defer lw.mu.Unlock()

	if lw.written >= lw.maxSize {
		return len(p), nil // Discard but report success
	}

	remaining := lw.maxSize - lw.written
	toWrite := int64(len(p))
	toWrite = min(toWrite, remaining)

	n, err = lw.buf.Write(p[:toWrite])
	lw.written += int64(n)

	if err == nil && int64(n) < int64(len(p)) {
		return len(p), nil // Report full write even if truncated
	}
	return n, err
}

type RunShellArgs struct {
	Cmd     string `json:"cmd" jsonschema:"The shell command to execute"`
	Cwd     string `json:"cwd,omitempty" jsonschema:"The working directory to execute the command in (default: \".\")"`
	Timeout int    `json:"timeout,omitempty" jsonschema:"Command execution timeout in seconds (default: 30)"`
}

type RunShellBackgroundArgs struct {
	Cmd string `json:"cmd" jsonschema:"The shell command to execute in the background"`
	Cwd string `json:"cwd,omitempty" jsonschema:"The working directory to execute the command in (default: \".\")"`
}

type ViewBackgroundJobArgs struct {
	JobID string `json:"job_id" jsonschema:"The ID of the background job to view"`
}

type StopBackgroundJobArgs struct {
	JobID string `json:"job_id" jsonschema:"The ID of the background job to stop"`
}

// statusStrings maps job status constants to their string representations
var statusStrings = map[int32]string{
	statusRunning:   "running",
	statusCompleted: "completed",
	statusStopped:   "stopped",
	statusFailed:    "failed",
}

// statusToString converts job status constant to string
func statusToString(status int32) string {
	if s, ok := statusStrings[status]; ok {
		return s
	}
	return "unknown"
}

// BackgroundJobInfo represents structured information about a background job.
type BackgroundJobInfo struct {
	ID       string `json:"id" jsonschema:"The unique job ID"`
	Command  string `json:"command" jsonschema:"The command being executed"`
	Cwd      string `json:"cwd" jsonschema:"The working directory"`
	Status   string `json:"status" jsonschema:"Job status: running, completed, stopped, or failed"`
	Runtime  string `json:"runtime" jsonschema:"How long the job has been running"`
	ExitCode *int   `json:"exitCode,omitempty" jsonschema:"Exit code if job has finished"`
	Output   string `json:"output,omitempty" jsonschema:"Job output (only in view_background_job)"`
}

func (h *shellHandler) RunShell(ctx context.Context, params RunShellArgs) (*tools.ToolCallResult, error) {
	// Determine effective timeout
	effectiveTimeout := h.timeout
	if params.Timeout > 0 {
		effectiveTimeout = time.Duration(params.Timeout) * time.Second
	}

	// Create timeout context
	timeoutCtx, cancel := context.WithTimeout(ctx, effectiveTimeout)
	defer cancel()

	cmd := exec.Command(h.shell, append(h.shellArgsPrefix, params.Cmd)...)
	cmd.Env = h.env
	cmd.Dir = params.Cwd
	if params.Cwd == "" || params.Cwd == "." {
		cmd.Dir = h.workingDir
	}

	cmd.SysProcAttr = platformSpecificSysProcAttr()

	var outBuf bytes.Buffer
	cmd.Stdout = &outBuf
	cmd.Stderr = &outBuf

	if err := cmd.Start(); err != nil {
		return tools.ResultError(fmt.Sprintf("Error starting command: %s", err)), nil
	}

	pg, err := createProcessGroup(cmd.Process)
	if err != nil {
		return tools.ResultError(fmt.Sprintf("Error creating process group: %s", err)), nil
	}

	done := make(chan error, 1)
	go func() {
		done <- cmd.Wait()
	}()

	var output string
	select {
	case <-timeoutCtx.Done():
		if cmd.Process != nil {
			_ = kill(cmd.Process, pg)
		}

		if ctx.Err() != nil {
			output = "Command cancelled"
		} else {
			output = fmt.Sprintf("Command timed out after %v\nOutput: %s", effectiveTimeout, outBuf.String())
		}
	case err := <-done:
		output = outBuf.String()

		if err != nil {
			output = fmt.Sprintf("Error executing command: %s\nOutput: %s", err, output)
		}
	}

	output = cmp.Or(strings.TrimSpace(output), "<no output>")

	return tools.ResultSuccess(limitOutput(output)), nil
}

func (h *shellHandler) RunShellBackground(_ context.Context, params RunShellBackgroundArgs) (*tools.ToolCallResult, error) {
	// Generate unique job ID
	counter := h.jobCounter.Add(1)
	jobID := fmt.Sprintf("job_%d_%d", time.Now().Unix(), counter)

	// Setup command (no context - background jobs run independently)
	cmd := exec.Command(h.shell, append(h.shellArgsPrefix, params.Cmd)...)
	cmd.Env = h.env
	cmd.Dir = params.Cwd
	if params.Cwd == "" || params.Cwd == "." {
		cmd.Dir = h.workingDir
	}

	cmd.SysProcAttr = platformSpecificSysProcAttr()

	// Create output buffer with 10MB limit
	outputBuf := &bytes.Buffer{}
	limitedWriter := &limitedWriter{buf: outputBuf, maxSize: 10 * 1024 * 1024}
	cmd.Stdout = limitedWriter
	cmd.Stderr = limitedWriter

	// Start the command
	if err := cmd.Start(); err != nil {
		return tools.ResultError(fmt.Sprintf("Error starting background command: %s", err)), nil
	}

	// Create process group
	pg, err := createProcessGroup(cmd.Process)
	if err != nil {
		_ = kill(cmd.Process, pg)
		return tools.ResultError(fmt.Sprintf("Error creating process group: %s", err)), nil
	}

	// Create and store job
	job := &backgroundJob{
		id:           jobID,
		cmd:          params.Cmd,
		cwd:          params.Cwd,
		process:      cmd.Process,
		processGroup: pg,
		output:       outputBuf,
		startTime:    time.Now(),
	}
	job.status.Store(statusRunning)
	h.jobs.Store(jobID, job)

	// Monitor job completion in background
	go func() {
		err := cmd.Wait()

		job.outputMu.Lock()
		defer job.outputMu.Unlock()

		// Don't overwrite if already stopped
		if job.status.Load() == statusStopped {
			return
		}

		if err != nil {
			if exitErr, ok := err.(*exec.ExitError); ok {
				job.exitCode = exitErr.ExitCode()
				job.status.Store(statusFailed)
			} else {
				job.exitCode = -1
				job.status.Store(statusFailed)
			}
			job.err = err
		} else {
			job.exitCode = 0
			job.status.Store(statusCompleted)
		}
	}()

	return tools.ResultSuccess(fmt.Sprintf("Background job started with ID: %s\nCommand: %s\nWorking directory: %s",
		jobID, params.Cmd, params.Cwd)), nil
}

func (h *shellHandler) ListBackgroundJobs(_ context.Context, _ map[string]any) (*tools.ToolCallResult, error) {
	var jobs []BackgroundJobInfo

	h.jobs.Range(func(_ string, job *backgroundJob) bool {
		status := job.status.Load()
		statusStr := statusToString(status)
		elapsed := time.Since(job.startTime).Round(time.Second)

		info := BackgroundJobInfo{
			ID:      job.id,
			Command: job.cmd,
			Cwd:     job.cwd,
			Status:  statusStr,
			Runtime: elapsed.String(),
		}

		if status != statusRunning {
			job.outputMu.RLock()
			exitCode := job.exitCode
			job.outputMu.RUnlock()
			info.ExitCode = &exitCode
		}

		jobs = append(jobs, info)
		return true
	})

	var output strings.Builder
	output.WriteString("Background Jobs:\n\n")

	if len(jobs) == 0 {
		output.WriteString("No background jobs found.\n")
	} else {
		for _, info := range jobs {
			fmt.Fprintf(&output, "ID: %s\n", info.ID)
			fmt.Fprintf(&output, "  Command: %s\n", info.Command)
			fmt.Fprintf(&output, "  Status: %s\n", info.Status)
			fmt.Fprintf(&output, "  Runtime: %s\n", info.Runtime)
			if info.ExitCode != nil {
				fmt.Fprintf(&output, "  Exit Code: %d\n", *info.ExitCode)
			}
			output.WriteString("\n")
		}
	}

	return &tools.ToolCallResult{
		Output: output.String(),
		Meta:   jobs,
	}, nil
}

func (h *shellHandler) ViewBackgroundJob(_ context.Context, params ViewBackgroundJobArgs) (*tools.ToolCallResult, error) {
	job, exists := h.jobs.Load(params.JobID)
	if !exists {
		return tools.ResultError(fmt.Sprintf("Job not found: %s", params.JobID)), nil
	}

	status := job.status.Load()
	statusStr := statusToString(status)
	elapsed := time.Since(job.startTime).Round(time.Second)

	job.outputMu.RLock()
	output := job.output.String()
	exitCode := job.exitCode
	job.outputMu.RUnlock()

	info := BackgroundJobInfo{
		ID:      job.id,
		Command: job.cmd,
		Cwd:     job.cwd,
		Status:  statusStr,
		Runtime: elapsed.String(),
		Output:  output,
	}

	if status != statusRunning {
		info.ExitCode = &exitCode
	}

	var result strings.Builder
	fmt.Fprintf(&result, "Job ID: %s\n", info.ID)
	fmt.Fprintf(&result, "Command: %s\n", info.Command)
	fmt.Fprintf(&result, "Status: %s\n", info.Status)
	fmt.Fprintf(&result, "Runtime: %s\n", info.Runtime)
	if info.ExitCode != nil {
		fmt.Fprintf(&result, "Exit Code: %d\n", *info.ExitCode)
	}
	result.WriteString("\n--- Output ---\n")
	if output == "" {
		result.WriteString("<no output>\n")
	} else {
		result.WriteString(output)
		if len(output) >= 10*1024*1024 {
			result.WriteString("\n\n[Output truncated at 10MB limit]")
		}
	}

	return &tools.ToolCallResult{
		Output: result.String(),
		Meta:   info,
	}, nil
}

func (h *shellHandler) StopBackgroundJob(_ context.Context, params StopBackgroundJobArgs) (*tools.ToolCallResult, error) {
	job, exists := h.jobs.Load(params.JobID)
	if !exists {
		return tools.ResultError(fmt.Sprintf("Job not found: %s", params.JobID)), nil
	}

	// Try to transition from running to stopped
	if !job.status.CompareAndSwap(statusRunning, statusStopped) {
		currentStatus := job.status.Load()
		statusStr := statusToString(currentStatus)
		return tools.ResultError(fmt.Sprintf("Job %s is not running (current status: %s)", params.JobID, statusStr)), nil
	}

	// Kill the process
	if err := kill(job.process, job.processGroup); err != nil {
		return tools.ResultError(fmt.Sprintf("Job %s marked as stopped, but error killing process: %s", params.JobID, err)), nil
	}

	return tools.ResultSuccess(fmt.Sprintf("Job %s stopped successfully", params.JobID)), nil
}

func NewShellTool(env []string, runConfig *config.RuntimeConfig) *ShellTool {
	var shell string
	var argsPrefix []string

	if runtime.GOOS == "windows" {
		// Prefer PowerShell (pwsh or Windows PowerShell) when available, otherwise fall back to cmd.exe
		if path, err := exec.LookPath("pwsh.exe"); err == nil {
			shell = path
			argsPrefix = []string{"-NoProfile", "-NonInteractive", "-Command"}
		} else if path, err := exec.LookPath("powershell.exe"); err == nil {
			shell = path
			argsPrefix = []string{"-NoProfile", "-NonInteractive", "-Command"}
		} else {
			// Use ComSpec if available, otherwise default to cmd.exe
			if comspec := os.Getenv("ComSpec"); comspec != "" {
				shell = comspec
			} else {
				shell = "cmd.exe"
			}
			argsPrefix = []string{"/C"}
		}
	} else {
		// Unix-like: use SHELL or default to /bin/sh
		shell = cmp.Or(os.Getenv("SHELL"), "/bin/sh")
		argsPrefix = []string{"-c"}
	}

	return &ShellTool{
		handler: &shellHandler{
			shell:           shell,
			shellArgsPrefix: argsPrefix,
			env:             env,
			timeout:         30 * time.Second,
			jobs:            concurrent.NewMap[string, *backgroundJob](),
			workingDir:      runConfig.WorkingDir,
		},
	}
}

func (t *ShellTool) Instructions() string {
	return `# Shell Tool Usage Guide

Execute shell commands in the user's environment with full control over working directories and command parameters.

## Core Concepts

**Execution Context**: Commands run in the user's default shell with access to all environment variables and the current workspace.
On Windows, PowerShell (pwsh/powershell) is used when available; otherwise, cmd.exe is used.
On Unix-like systems, ${SHELL} is used or /bin/sh as fallback.

**Working Directory Management**:
- Default execution location: working directory of the agent
- Override with "cwd" parameter for targeted command execution
- Supports both absolute and relative paths

**Command Isolation**: Each tool call creates a fresh shell session - no state persists between executions.

**Timeout Protection**: Commands have a default 30-second timeout to prevent hanging. For longer operations, specify a custom timeout.

## Parameter Reference

| Parameter | Type   | Required | Description |
|-----------|--------|----------|-------------|
| cmd       | string | Yes      | Shell command to execute |
| cwd       | string | Yes      | Working directory (use "." for current) |
| timeout   | int    | No       | Timeout in seconds (default: 30) |

## Best Practices

### ✅ DO
- Leverage the "cwd" parameter for directory-specific commands, rather than cding within commands
- Quote arguments containing spaces or special characters
- Use pipes and redirections
- Write advanced scripts with heredocs, that replace a lot of simple commands or tool calls
- This tool is great at reading and writing multiple files at once
- Avoid writing shell scripts to the disk. Instead, use heredocs to pipe the script to the SHELL
- Use the timeout parameter for long-running operations (e.g., builds, tests)

### Git Commits

When user asks to create git commit

- Add "Assisted-By: cagent" as a trailer line in the commit message
- Use the format: git commit -m "Your commit message" -m "" -m "Assisted-By: cagent"

## Usage Examples

**Basic command execution:**
{ "cmd": "ls -la", "cwd": "." }

**Long-running command with custom timeout:**
{ "cmd": "npm run build", "cwd": ".", "timeout": 120 }

**Language-specific operations:**
{ "cmd": "go test ./...", "cwd": ".", "timeout": 180 }
{ "cmd": "npm install", "cwd": "frontend" }
{ "cmd": "python -m pytest tests/", "cwd": "backend", "timeout": 90 }

**File operations:**
{ "cmd": "find . -name '*.go' -type f", "cwd": "." }
{ "cmd": "grep -r 'TODO' src/", "cwd": "." }

**Process management:**
{ "cmd": "ps aux | grep node", "cwd": "." }
{ "cmd": "docker ps --format 'table {{.Names}}\t{{.Status}}'", "cwd": "." }

**Complex pipelines:**
{ "cmd": "cat package.json | jq '.dependencies'", "cwd": "frontend" }

**Bash scripts:**
{ "cmd": "cat << 'EOF' | ${SHELL}
echo Hello
EOF" }

## Error Handling

Commands that exit with non-zero status codes will return error information along with any output produced before failure.
Commands that exceed their timeout will be terminated automatically.

---

# Background Jobs

Run long-running processes in the background while continuing with other tasks. Perfect for starting servers, watching files, or any process that needs to run alongside other operations.

## When to Use Background Jobs

**Use background jobs for:**
- Starting web servers, databases, or other services
- Running file watchers or live reload tools
- Long-running processes that other tasks depend on
- Commands that produce continuous output over time

**Don't use background jobs for:**
- Quick commands that complete in seconds
- Commands where you need immediate results
- One-time operations (use regular shell tool instead)

## Background Job Tools

**run_background_job**: Start a command in the background
- Parameters: cmd (required), cwd (optional, defaults to ".")
- Returns: Job ID for tracking

**list_background_jobs**: List all background jobs
- No parameters required
- Returns: Status, runtime, and command for each job

**view_background_job**: View output of a specific job
- Parameters: job_id (required)
- Returns: Current output and job status

**stop_background_job**: Stop a running job
- Parameters: job_id (required)
- Terminates the process and all child processes

## Background Job Workflow

**1. Start a background job:**
{ "cmd": "npm start", "cwd": "frontend" }
→ Returns job ID (e.g., "job_1731772800_1")

**2. Check running jobs:**
Use list_background_jobs to see all jobs with their status

**3. View job output:**
{ "job_id": "job_1731772800_1" }
→ Shows current output and status

**4. Stop job when done:**
{ "job_id": "job_1731772800_1" }
→ Terminates the process and all child processes

## Important Characteristics

**Output Buffering**: Each job captures up to 10MB of output. Beyond this limit, new output is discarded to prevent memory issues.

**Process Groups**: Background jobs and all their child processes are managed as a group. Stopping a job terminates the entire process tree.

**Environment Inheritance**: Jobs inherit environment variables from when they are started. Changes after job start don't affect running jobs.

**Automatic Cleanup**: All background jobs are automatically terminated when the agent stops.

**Job Persistence**: Job history is kept in memory until agent stops. Completed jobs remain queryable.

## Background Job Examples

**Start a web server:**
{ "cmd": "python -m http.server 8000", "cwd": "." }

**Start a development server:**
{ "cmd": "npm run dev", "cwd": "frontend" }

**Run a file watcher:**
{ "cmd": "go run . watch", "cwd": "." }

**Start a database:**
{ "cmd": "docker run --rm -p 5432:5432 postgres:latest", "cwd": "." }

**Multiple services pattern:**
1. Start backend: run_background_job with server command
2. Start frontend: run_background_job with dev server
3. Perform tasks: use other tools while services run
4. Check logs: view_background_job to see service output
5. Cleanup: stop_background_job for each service (or let agent cleanup automatically)`
}

func (t *ShellTool) Tools(context.Context) ([]tools.Tool, error) {
	return []tools.Tool{
		{
			Name:         ToolNameShell,
			Category:     "shell",
			Description:  `Executes the given shell command in the user's default shell.`,
			Parameters:   tools.MustSchemaFor[RunShellArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handler.RunShell),
			Annotations: tools.ToolAnnotations{
				Title: "Shell",
			},
		},
		{
			Name:         ToolNameRunShellBackground,
			Category:     "shell",
			Description:  `Starts a shell command in the background and returns immediately with a job ID. Use this for long-running processes like servers, watches, or any command that should run while other tasks are performed.`,
			Parameters:   tools.MustSchemaFor[RunShellBackgroundArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handler.RunShellBackground),
			Annotations: tools.ToolAnnotations{
				Title: "Background Job",
			},
		},
		{
			Name:                 ToolNameListBackgroundJobs,
			Category:             "shell",
			Description:          `Lists all background jobs with their status, runtime, and other information.`,
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[[]BackgroundJobInfo](),
			Handler:              NewHandler(t.handler.ListBackgroundJobs),
			Annotations: tools.ToolAnnotations{
				Title:        "List Background Jobs",
				ReadOnlyHint: true,
			},
		},
		{
			Name:                 ToolNameViewBackgroundJob,
			Category:             "shell",
			Description:          `Views the output and status of a specific background job by job ID.`,
			Parameters:           tools.MustSchemaFor[ViewBackgroundJobArgs](),
			OutputSchema:         tools.MustSchemaFor[string](),
			CodeModeOutputSchema: tools.MustSchemaFor[BackgroundJobInfo](),
			Handler:              NewHandler(t.handler.ViewBackgroundJob),
			Annotations: tools.ToolAnnotations{
				Title:        "View Background Job Output",
				ReadOnlyHint: true,
			},
		},
		{
			Name:         ToolNameStopBackgroundJob,
			Category:     "shell",
			Description:  `Stops a running background job by job ID. The process and all its child processes will be terminated.`,
			Parameters:   tools.MustSchemaFor[StopBackgroundJobArgs](),
			OutputSchema: tools.MustSchemaFor[string](),
			Handler:      NewHandler(t.handler.StopBackgroundJob),
			Annotations: tools.ToolAnnotations{
				Title: "Stop Background Job",
			},
		},
	}, nil
}

func (t *ShellTool) Stop(context.Context) error {
	// Terminate all running background jobs
	t.handler.jobs.Range(func(_ string, job *backgroundJob) bool {
		if job.status.CompareAndSwap(statusRunning, statusStopped) {
			_ = kill(job.process, job.processGroup)
		}
		return true
	})
	return nil
}
