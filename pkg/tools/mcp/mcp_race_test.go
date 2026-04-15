package mcp

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
)

func TestInstructions_Concurrent(t *testing.T) {
	t.Parallel()

	ts := &Toolset{
		started:      true,
		instructions: "initial",
	}

	var wg sync.WaitGroup
	for i := 0; i < 100; i++ {
		wg.Add(2)
		go func() {
			defer wg.Done()
			// Simulate what doStart does (always called under ts.mu)
			ts.mu.Lock()
			ts.instructions = "updated"
			ts.mu.Unlock()
		}()
		go func() {
			defer wg.Done()
			_ = ts.Instructions()
		}()
	}
	wg.Wait()
}

func TestTryRestart_RespectsContextCancellation(t *testing.T) {
	t.Parallel()

	ts := &Toolset{
		logID: "test",
		mcpClient: &mockMCPClient{},
	}

	ctx, cancel := context.WithCancel(t.Context())

	done := make(chan bool, 1)
	go func() {
		done <- ts.tryRestart(ctx)
	}()

	// Cancel almost immediately; tryRestart should return promptly
	// instead of sleeping through the full backoff.
	time.Sleep(50 * time.Millisecond)
	cancel()

	select {
	case result := <-done:
		assert.False(t, result, "tryRestart should return false on cancellation")
	case <-time.After(2 * time.Second):
		t.Fatal("tryRestart did not return promptly after context cancellation")
	}
}
