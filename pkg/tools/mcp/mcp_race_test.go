package mcp

import (
	"sync"
	"testing"
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
