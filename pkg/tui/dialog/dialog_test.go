package dialog

import (
	"fmt"
	"testing"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

// TestContainsPoint_NoDialog verifies the manager reports no point as
// contained when the stack is empty.
func TestContainsPoint_NoDialog(t *testing.T) {
	t.Parallel()

	mgr := New()
	assert.False(t, mgr.ContainsPoint(0, 0))
	assert.False(t, mgr.ContainsPoint(50, 50))
}

// TestContainsPoint_WithDialog verifies the manager correctly reports points
// inside and outside an open dialog. The help dialog uses the screen size to
// compute its dimensions, so we feed in a known WindowSizeMsg first.
func TestContainsPoint_WithDialog(t *testing.T) {
	t.Parallel()

	mgr := New()

	// Initialize size and open a help dialog (a stable, simple dialog).
	updated, _ := mgr.Update(tea.WindowSizeMsg{Width: 120, Height: 40})
	mgr = updated.(Manager)
	updated, _ = mgr.Update(OpenDialogMsg{Model: NewHelpDialog(nil)})
	mgr = updated.(Manager)

	require.True(t, mgr.Open(), "dialog should be open")

	// Compute the actual rendered bounds of the dialog to make the test
	// independent of styling tweaks.
	layers := mgr.GetLayers()
	require.Len(t, layers, 1, "expected exactly one dialog layer")
	view := mgr.View()
	dialogW := lipgloss.Width(view)
	dialogH := lipgloss.Height(view)
	col := layers[0].GetX()
	row := layers[0].GetY()
	require.Positive(t, dialogW, "dialog should have non-zero width")
	require.Positive(t, dialogH, "dialog should have non-zero height")

	debug := fmt.Sprintf("dialog at row=%d col=%d size=%dx%d", row, col, dialogW, dialogH)

	// Negative coordinates always fall outside.
	assert.False(t, mgr.ContainsPoint(-1, -1), debug)

	// (0,0) sits in the screen corner — well outside the centred dialog.
	assert.False(t, mgr.ContainsPoint(0, 0), debug)

	// Point inside the dialog (its top-left corner).
	assert.True(t, mgr.ContainsPoint(col, row), debug)

	// Point at the dialog's last cell.
	assert.True(t, mgr.ContainsPoint(col+dialogW-1, row+dialogH-1), debug)

	// One column past the dialog's right edge falls outside.
	assert.False(t, mgr.ContainsPoint(col+dialogW, row), debug)
}

// TestContainsPoint_AfterClose verifies that closing the dialog removes it
// from hit-testing.
func TestContainsPoint_AfterClose(t *testing.T) {
	t.Parallel()

	mgr := New()
	updated, _ := mgr.Update(tea.WindowSizeMsg{Width: 120, Height: 40})
	mgr = updated.(Manager)
	updated, _ = mgr.Update(OpenDialogMsg{Model: NewHelpDialog(nil)})
	mgr = updated.(Manager)

	layers := mgr.GetLayers()
	require.Len(t, layers, 1)
	view := mgr.View()
	col := layers[0].GetX()
	row := layers[0].GetY()
	require.True(t, mgr.ContainsPoint(col, row), "sanity: point inside the dialog should match before close")
	_ = lipgloss.Width(view)

	updated, _ = mgr.Update(CloseDialogMsg{})
	mgr = updated.(Manager)
	assert.False(t, mgr.Open())
	assert.False(t, mgr.ContainsPoint(col, row),
		"after closing, no point should be reported as contained")
}
