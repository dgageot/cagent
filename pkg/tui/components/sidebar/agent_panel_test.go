package sidebar

import (
	"strings"
	"testing"

	"github.com/charmbracelet/x/ansi"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/runtime"
	"github.com/docker/docker-agent/pkg/session"
	"github.com/docker/docker-agent/pkg/tui/service"
	"github.com/docker/docker-agent/pkg/tui/styles"
)

// newAgentPanelSidebar builds a sidebar whose current agent and roster are set,
// ready to render the Agents panel at the given outer width.
func newAgentPanelSidebar(t *testing.T, current string, width int, agents ...runtime.AgentDetails) *model {
	t.Helper()
	sess := session.New()
	ss := service.NewSessionState(sess)
	ss.SetCurrentAgentName(current)
	m := New(t.Context(), ss).(*model)
	m.sessionHasContent = true
	m.titleGenerated = true
	m.sessionTitle = "Test"
	m.currentAgent = current
	m.availableAgents = agents
	m.width = width
	m.height = 200
	return m
}

// renderAgentPanel returns the ANSI-stripped lines of the Agents panel body.
func renderAgentPanel(m *model) []string {
	out := ansi.Strip(m.agentInfo(m.contentWidth(false)))
	return strings.Split(out, "\n")
}

func TestClassifyThinking(t *testing.T) {
	t.Parallel()

	cases := []struct {
		label    string
		wantKind thinkingKind
		wantTok  int64
	}{
		{"", thinkingNone, 0},
		{"off", thinkingOff, 0},
		{"adaptive", thinkingAdaptive, 0},
		{"8192", thinkingTokens, 8192},
		{"high", thinkingLevel, 0},
		{"minimal", thinkingLevel, 0},
	}
	for _, c := range cases {
		kind, tok := classifyThinking(c.label)
		assert.Equalf(t, c.wantKind, kind, "kind for %q", c.label)
		assert.Equalf(t, c.wantTok, tok, "tokens for %q", c.label)
	}
}

// TestCardThinkingLine covers every vocabulary case for the focus-card thinking
// line, including the empty case (line omitted) and the token-budget formatting.
func TestCardThinkingLine(t *testing.T) {
	t.Parallel()

	cases := []struct {
		label    string
		contains string
		omit     bool
	}{
		{"high", "thinking " + gaugePattern(4) + " high", false},
		{"adaptive", "thinking auto adaptive", false},
		{"8192", "thinking " + styles.TokenGlyph + " 8.2K tokens", false},
		{"off", "thinking " + gaugePattern(0) + " off", false},
		{"", "", true},
	}
	for _, c := range cases {
		got := ansi.Strip(cardThinkingLine(c.label))
		if c.omit {
			assert.Emptyf(t, got, "label %q should omit the card line", c.label)
			continue
		}
		assert.Equalf(t, c.contains, got, "label %q", c.label)
	}
}

// TestCardRenderedInPlace verifies the current agent renders as a focus card at
// its natural config-order position (not pinned to the top), with the wrapped
// description, full model id and thinking line, while the agents before it
// render as compact rows.
func TestCardRenderedInPlace(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "root", 40,
		runtime.AgentDetails{Name: "first", Provider: "openai", Model: "gpt-5.4-mini", Thinking: "off"},
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "claude-opus-4-8", Description: "Executive assistant", Thinking: "high"},
		runtime.AgentDetails{Name: "last", Provider: "google", Model: "gemini-flash", Thinking: "off"},
	)

	lines := renderAgentPanel(m)

	// Find the compact row for "first" and the card header for "root".
	firstRow := -1
	cardHeader := -1
	for i, l := range lines {
		if strings.Contains(l, "first") && strings.Contains(l, "^1") {
			firstRow = i
		}
		if strings.Contains(l, "▶ root") {
			cardHeader = i
		}
	}
	require.Positive(t, firstRow, "compact row for the first (non-current) agent should render")
	require.Positive(t, cardHeader, "card header for the current agent should render")
	assert.Less(t, firstRow, cardHeader, "card must render in place, after the first agent's row")

	body := strings.Join(lines, "\n")
	assert.Contains(t, body, "Executive assistant", "card shows description")
	assert.Contains(t, body, "anthropic/claude-opus-4-8", "card shows full provider/model")
	assert.Contains(t, body, "thinking "+gaugePattern(4)+" high", "card shows gauge+value thinking line")
}

// TestCardDescriptionWrapsToTwoLinesWithEllipsis verifies a long description is
// wrapped to at most two lines, with the second line ending in an ellipsis.
func TestCardDescriptionWrapsToTwoLinesWithEllipsis(t *testing.T) {
	t.Parallel()

	long := "This is a very long agent description that certainly will not fit on a single sidebar line and must wrap onto a second line and then be truncated"
	m := newAgentPanelSidebar(t, "root", 40,
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "claude-opus-4-8", Description: long, Thinking: "high"},
	)

	lines := renderAgentPanel(m)

	// Description lines are the wrapped body lines under the card.
	var descLines []string
	for _, l := range lines {
		if strings.Contains(l, "This is a very long") || strings.Contains(l, "that certainly") {
			descLines = append(descLines, l)
		}
	}
	require.Len(t, descLines, 2, "description must wrap to exactly two lines")
	assert.Contains(t, descLines[1], "…", "second description line ends with an ellipsis")
}

// TestHarnessCardNoThinkingLine verifies a harness-backed current agent (empty
// Thinking, harness type as model) shows the harness type and no thinking line.
func TestHarnessCardNoThinkingLine(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "slack", 40,
		runtime.AgentDetails{Name: "slack", Model: "claude-code", Description: "Slack agent", Thinking: ""},
	)

	body := strings.Join(renderAgentPanel(m), "\n")
	assert.Contains(t, body, "claude-code", "harness card shows the harness type as the model")
	assert.NotContains(t, body, "thinking:", "harness card has no thinking line")
}

// TestRowColumnAlignment verifies roster rows share an aligned badge column and
// that badges render with the expected vocabulary: effort levels become the
// fixed-width gauge while token/adaptive/off keep their text badges.
func TestRowColumnAlignment(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "root", 40,
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "opus", Thinking: "high"},
		runtime.AgentDetails{Name: "alpha", Provider: "openai", Model: "gpt-5.4-mini", Thinking: "off"},
		runtime.AgentDetails{Name: "beta", Provider: "openai", Model: "gpt-5.4", Thinking: "high"},
		runtime.AgentDetails{Name: "gamma", Provider: "openai", Model: "gpt-4o", Thinking: "8192"},
		runtime.AgentDetails{Name: "delta", Provider: "google", Model: "gemini", Thinking: "adaptive"},
	)

	lines := renderAgentPanel(m)

	type rowBadge struct{ line, badge string }
	var rows []rowBadge
	for _, l := range lines {
		switch {
		case strings.Contains(l, "alpha"):
			rows = append(rows, rowBadge{l, gaugePattern(0)}) // off → empty gauge
		case strings.Contains(l, "beta"):
			rows = append(rows, rowBadge{l, gaugePattern(4)}) // high → six-cell gauge
		case strings.Contains(l, "gamma"):
			rows = append(rows, rowBadge{l, styles.TokenGlyph + " 8.2K"})
		case strings.Contains(l, "delta"):
			rows = append(rows, rowBadge{l, "auto"})
		}
	}
	require.Len(t, rows, 4, "all four non-current rows should render")

	for _, r := range rows {
		assert.Truef(t, strings.HasSuffix(strings.TrimRight(r.line, " "), r.badge), "row %q should end with badge %q", r.line, r.badge)
	}

	// Badges are right-aligned: every row is padded to the same total width, so
	// the trimmed rows share the same rune length (badges end at one column).
	end := -1
	for _, r := range rows {
		require.GreaterOrEqual(t, strings.LastIndex(r.line, r.badge), 0)
		w := len([]rune(strings.TrimRight(r.line, " ")))
		if end == -1 {
			end = w
		} else {
			assert.Equal(t, end, w, "right-aligned badges must end in a single column")
		}
	}
}

// TestRowModelLeftTruncated verifies the model column keeps its informative tail
// (left-truncation with a leading ellipsis) when it overflows.
func TestRowModelLeftTruncated(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "root", 30,
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "opus", Thinking: "high"},
		runtime.AgentDetails{Name: "agent2", Provider: "anthropic", Model: "claude-sonnet-4-6", Thinking: "off"},
	)

	lines := renderAgentPanel(m)
	var model string
	for i, l := range lines {
		if strings.Contains(l, "agent2") && i+1 < len(lines) {
			model = lines[i+1] // the provider/model is the row's second line
		}
	}
	require.NotEmpty(t, model)
	assert.Contains(t, model, "…", "overflowing model is left-truncated with an ellipsis")
	assert.Contains(t, model, "-4-6", "informative model tail survives left-truncation")
}

// TestRowDigitsRenderTokenBadge verifies a digits-only wire label produces a
// token badge with the token glyph and formatted count.
func TestRowDigitsRenderTokenBadge(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "root", 40,
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "opus", Thinking: "high"},
		runtime.AgentDetails{Name: "gateway", Provider: "openai", Model: "gpt-5.4", Thinking: "8192"},
	)

	body := strings.Join(renderAgentPanel(m), "\n")
	assert.Contains(t, body, styles.TokenGlyph+" 8.2K", "digits label renders a token badge")
}

// TestHarnessRowNoBadge verifies a harness row shows the harness type as the
// model and no thinking badge.
func TestHarnessRowNoBadge(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "root", 40,
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "opus", Thinking: "high"},
		runtime.AgentDetails{Name: "slack", Model: "claude-code", Thinking: ""},
	)

	lines := renderAgentPanel(m)
	var line1, line2 string
	for i, l := range lines {
		if strings.Contains(l, "slack") {
			line1 = l
			if i+1 < len(lines) {
				line2 = lines[i+1]
			}
		}
	}
	require.NotEmpty(t, line1)
	assert.Contains(t, line2, "claude-code", "harness row shows the harness type on its model line")
	assert.NotContains(t, line1, styles.GaugeFilled, "harness row has no thinking gauge")
	assert.NotContains(t, line1, styles.GaugeEmpty, "harness row has no thinking gauge")
	assert.NotContains(t, line1, styles.TokenGlyph, "harness row has no token badge")
}

// TestMoreThanNineAgentsNoShortcutBeyond9 verifies agents past the 9th get no
// "^N" shortcut hint.
func TestMoreThanNineAgentsNoShortcutBeyond9(t *testing.T) {
	t.Parallel()

	agents := []runtime.AgentDetails{
		{Name: "root", Provider: "anthropic", Model: "opus", Thinking: "high"},
	}
	for i := 2; i <= 12; i++ {
		agents = append(agents, runtime.AgentDetails{
			Name:     "agent" + string(rune('a'+i)),
			Provider: "openai",
			Model:    "gpt-4o",
			Thinking: "off",
		})
	}
	m := newAgentPanelSidebar(t, "root", 40, agents...)

	body := strings.Join(renderAgentPanel(m), "\n")
	assert.Contains(t, body, "^9", "the 9th agent keeps its shortcut")
	assert.NotContains(t, body, "^10", "agents beyond the 9th have no shortcut")
}

// TestDegradationLadder verifies the two-line roster degrades by available
// width: the provider/model always occupies line 2, while line 1's thinking
// gauge collapses from the full six-cell gauge to a single cell near MinWidth.
func TestDegradationLadder(t *testing.T) {
	t.Parallel()

	makeModel := func(width int) *model {
		return newAgentPanelSidebar(t, "root", width,
			runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "opus", Thinking: "high"},
			runtime.AgentDetails{Name: "agent2", Provider: "anthropic", Model: "claude-sonnet-4-6", Thinking: "off"},
		)
	}

	linesFor := func(m *model, name string) (line1, line2 string) {
		ls := renderAgentPanel(m)
		for i, l := range ls {
			if strings.Contains(l, name) {
				line1 = l
				if i+1 < len(ls) {
					line2 = ls[i+1]
				}
				return line1, line2
			}
		}
		return "", ""
	}

	// Wide: full empty gauge on line 1, full model on line 2.
	w1, w2 := linesFor(makeModel(40), "agent2")
	assert.Contains(t, w1, gaugePattern(0), "wide layout shows the full empty gauge for off")
	assert.Contains(t, w2, "sonnet-4-6", "wide layout shows the full model on line 2")

	// Near MinWidth (content < rowGlyphOnlyMinWidth): line-1 gauge collapses to a
	// single cell; the model on line 2 truncates with a leading ellipsis.
	n1, n2 := linesFor(makeModel(21), "agent2")
	assert.NotContains(t, n1, gaugePattern(0), "glyph-only layout collapses the full gauge")
	assert.Contains(t, n1, styles.GaugeEmpty, "glyph-only layout keeps a single gauge cell")
	assert.Contains(t, n2, "…", "narrow layout left-truncates the model on line 2")
}

// TestClickZonesCardAndRow verifies that clicking a card line and clicking a row
// line both resolve to the correct agent.
func TestClickZonesCardAndRow(t *testing.T) {
	t.Parallel()

	sess := session.New()
	ss := service.NewSessionState(sess)
	ss.SetCurrentAgentName("root")
	sb := New(t.Context(), ss)
	m := sb.(*model)
	m.sessionHasContent = true
	m.titleGenerated = true
	m.sessionTitle = "Test"
	m.currentAgent = "root"
	m.availableAgents = []runtime.AgentDetails{
		{Name: "first", Provider: "openai", Model: "gpt-5.4-mini", Thinking: "off"},
		{Name: "root", Provider: "anthropic", Model: "claude-opus-4-8", Description: "Executive assistant", Thinking: "high"},
	}
	m.width = 40
	m.height = 200

	_ = sb.View()

	paddingLeft := m.layoutCfg.PaddingLeft
	foundCard := false
	foundRow := false
	for y := range len(m.cachedLines) {
		result, name := sb.HandleClickType(paddingLeft+2, y)
		if result != ClickAgent {
			continue
		}
		if name == "root" {
			foundCard = true
		}
		if name == "first" {
			foundRow = true
		}
	}
	assert.True(t, foundCard, "clicking a card line should switch to the card's agent")
	assert.True(t, foundRow, "clicking a row line should switch to the row's agent")
}

// TestRosterSeparatesAgentsWithBlankLine verifies a blank separator line is
// inserted between agent entries (so the two-line rows and the card don't blend
// together) and that the separator carries an empty owner: a roster agent owns
// exactly its two content lines, never the separator, so click zones stay
// aligned.
func TestRosterSeparatesAgentsWithBlankLine(t *testing.T) {
	t.Parallel()

	m := newAgentPanelSidebar(t, "root", 40,
		runtime.AgentDetails{Name: "root", Provider: "anthropic", Model: "opus", Description: "Lead", Thinking: "high"},
		runtime.AgentDetails{Name: "alpha", Provider: "openai", Model: "gpt-5.4-mini", Thinking: "off"},
		runtime.AgentDetails{Name: "beta", Provider: "openai", Model: "gpt-5.4", Thinking: "high"},
	)

	_ = renderAgentPanel(m) // populates agentLineOwners

	// Each non-current roster agent owns exactly its two content lines; the
	// separators between entries are owned by nobody.
	counts := map[string]int{}
	blanks := 0
	for _, owner := range m.agentLineOwners {
		if owner == "" {
			blanks++
			continue
		}
		counts[owner]++
	}
	assert.Equal(t, 2, counts["alpha"], "a roster agent owns exactly its two content lines, not the separator")
	assert.Equal(t, 2, counts["beta"], "a roster agent owns exactly its two content lines, not the separator")
	assert.Positive(t, blanks, "agents are separated by blank, unowned lines")

	// A blank separator immediately precedes each non-first agent's first owned
	// line, and no separator leads the panel.
	require.NotEmpty(t, m.agentLineOwners)
	assert.NotEmpty(t, m.agentLineOwners[0], "the panel does not start with a separator")
	alphaStart := -1
	for i, owner := range m.agentLineOwners {
		if owner == "alpha" {
			alphaStart = i
			break
		}
	}
	require.Positive(t, alphaStart, "alpha should own lines after the card")
	assert.Empty(t, m.agentLineOwners[alphaStart-1], "a blank separator precedes the alpha entry")
}
