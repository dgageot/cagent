package tui

import (
	"testing"

	tea "charm.land/bubbletea/v2"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/tui/components/tabbar"
	"github.com/docker/docker-agent/pkg/tui/dialog"
	"github.com/docker/docker-agent/pkg/tui/messages"
)

// dialogBypassFixture wires up a minimal appModel with a tabbar holding two
// tabs and an open help dialog. It is shared by the tests below so each one
// can focus on a single aspect of the dialog-bypass behaviour added in
// #2626.
func dialogBypassFixture(t *testing.T) *appModel {
	t.Helper()

	m, _ := newTestModel()

	// A tab bar with two tabs so Ctrl+n / Ctrl+p have somewhere to go.
	tb := tabbar.New(20)
	tb.SetWidth(80)
	tb.SetTabs([]messages.TabInfo{
		{SessionID: "tab-a", Title: "A"},
		{SessionID: "tab-b", Title: "B"},
	}, 0)
	m.tabBar = tb
	m.width = 120
	m.height = 40

	// Initialise the dialog manager's window size, then open a dialog so
	// m.dialogMgr.Open() is true for the rest of the test.
	updated, _ := m.dialogMgr.Update(tea.WindowSizeMsg{Width: m.width, Height: m.height})
	m.dialogMgr = updated.(dialog.Manager)
	updated, _ = m.dialogMgr.Update(dialog.OpenDialogMsg{Model: dialog.NewHelpDialog(nil)})
	m.dialogMgr = updated.(dialog.Manager)
	require.True(t, m.dialogMgr.Open(), "fixture should leave a dialog open")

	return m
}

// TestKeyPress_TabNavigationBypassesOpenDialog verifies that Ctrl+n is
// forwarded to the tab bar (producing a SwitchTabMsg) instead of being
// absorbed by the open dialog. This is the headline fix for #2626.
func TestKeyPress_TabNavigationBypassesOpenDialog(t *testing.T) {
	t.Parallel()

	m := dialogBypassFixture(t)

	_, cmd := m.handleKeyPress(tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})
	require.NotNil(t, cmd, "Ctrl+n with two tabs should produce a SwitchTabMsg")

	msgs := collectMsgs(cmd)
	assert.True(t, hasMsg[messages.SwitchTabMsg](msgs),
		"Ctrl+n with an open dialog should still switch tabs (#2626)")

	// The dialog must remain open — switching tabs only swaps the active
	// dialog stack; it does not dismiss the dialog on the originating tab.
	assert.True(t, m.dialogMgr.Open(),
		"tab navigation must not dismiss an open dialog")
}

// TestKeyPress_NewTabBypassesOpenDialog verifies Ctrl+t is forwarded to the
// tab bar even with a dialog open.
func TestKeyPress_NewTabBypassesOpenDialog(t *testing.T) {
	t.Parallel()

	m := dialogBypassFixture(t)

	_, cmd := m.handleKeyPress(tea.KeyPressMsg{Code: 't', Mod: tea.ModCtrl})
	require.NotNil(t, cmd, "Ctrl+t should always produce a SpawnSessionMsg")

	msgs := collectMsgs(cmd)
	assert.True(t, hasMsg[messages.SpawnSessionMsg](msgs),
		"Ctrl+t with an open dialog should still spawn a new session (#2626)")
}

// TestKeyPress_NonTabKeyForwardsToDialog verifies that ordinary key presses
// (e.g. a printable character) are still absorbed by the open dialog and not
// re-routed to the tab bar.
func TestKeyPress_NonTabKeyForwardsToDialog(t *testing.T) {
	t.Parallel()

	m := dialogBypassFixture(t)

	_, cmd := m.handleKeyPress(tea.KeyPressMsg{Code: 'a'})

	for _, msg := range collectMsgs(cmd) {
		assert.IsNotType(t, messages.SwitchTabMsg{}, msg)
		assert.IsNotType(t, messages.SpawnSessionMsg{}, msg)
		assert.IsNotType(t, messages.CloseTabMsg{}, msg)
	}
}

// TestKeyPress_CloseTabIsForwardedToDialog verifies that Ctrl+w is *not*
// part of the bypass set: it is handled by the dialog (so the user can,
// say, focus a button) rather than destructively closing the tab and
// orphaning a pending elicitation. The user can dismiss the dialog with
// Esc first if they want to close the tab.
func TestKeyPress_CloseTabIsForwardedToDialog(t *testing.T) {
	t.Parallel()

	m := dialogBypassFixture(t)

	_, cmd := m.handleKeyPress(tea.KeyPressMsg{Code: 'w', Mod: tea.ModCtrl})

	for _, msg := range collectMsgs(cmd) {
		assert.IsNotType(t, messages.CloseTabMsg{}, msg,
			"Ctrl+w with a dialog open must NOT close the tab (#2626)")
	}
}

// TestWheelCoalesced_OutsideDialogScrollsChat verifies that a mouse wheel
// event whose coordinates fall outside the open dialog's bounds is routed
// to the chat content region instead of being absorbed by the dialog.
// This lets the user scroll the conversation while a non-modal prompt is
// waiting for input (#2626).
func TestWheelCoalesced_OutsideDialogScrollsChat(t *testing.T) {
	t.Parallel()

	m := dialogBypassFixture(t)
	m.contentHeight = 30 // large enough that y=0 lands in the content region

	// Sanity: the help dialog is centred, so (0, 0) is outside its bounds.
	require.False(t, m.dialogMgr.ContainsPoint(0, 0),
		"sanity: (0,0) should be outside the centred help dialog")

	_, _ = m.handleWheelCoalesced(messages.WheelCoalescedMsg{X: 0, Y: 0, Delta: -1})

	// The dialog must remain open after a wheel event passes through.
	assert.True(t, m.dialogMgr.Open(),
		"wheel events outside the dialog must not dismiss it")
}

// TestWheelCoalesced_OverDialogIsForwardedToDialog verifies that a wheel
// event whose coordinates fall over the open dialog is still forwarded to
// the dialog (so the dialog's own scrolling continues to work).
func TestWheelCoalesced_OverDialogIsForwardedToDialog(t *testing.T) {
	t.Parallel()

	m := dialogBypassFixture(t)

	// Find a point that lies inside the dialog.
	layers := m.dialogMgr.GetLayers()
	require.Len(t, layers, 1)
	col := layers[0].GetX()
	row := layers[0].GetY()
	require.True(t, m.dialogMgr.ContainsPoint(col, row),
		"sanity: dialog corner should be inside the dialog")

	_, _ = m.handleWheelCoalesced(messages.WheelCoalescedMsg{X: col, Y: row, Delta: -1})

	assert.True(t, m.dialogMgr.Open(),
		"wheel events over the dialog must keep it open")
}
