package chat

import (
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
)

func TestMergeCitations_DeduplicatesByURIPreservingOrder(t *testing.T) {
	t.Parallel()

	merged, added := MergeCitations(
		[]Citation{{URI: "https://a.example", Title: "A"}},
		[]Citation{
			{URI: "https://b.example", Title: "B"},
			{URI: "https://a.example", Title: "A again"},
			{URI: "https://b.example"},
			{URI: "https://c.example"},
		},
	)

	assert.Equal(t, []Citation{
		{URI: "https://a.example", Title: "A"},
		{URI: "https://b.example", Title: "B"},
		{URI: "https://c.example"},
	}, merged)
	assert.Equal(t, []Citation{{URI: "https://b.example", Title: "B"}, {URI: "https://c.example"}}, added)
}

func TestMergeCitations_NilInputs(t *testing.T) {
	t.Parallel()

	merged, added := MergeCitations(nil, nil)
	assert.Nil(t, merged)
	assert.Nil(t, added)

	merged, added = MergeCitations(nil, []Citation{{Title: "no uri"}, {URI: "   "}})
	assert.Nil(t, merged, "citations without a usable URI are dropped")
	assert.Nil(t, added)
}

func TestSanitizeCitation(t *testing.T) {
	t.Parallel()

	tests := []struct {
		name string
		in   Citation
		want Citation
		ok   bool
	}{
		{name: "plain", in: Citation{URI: " https://a.example/x ", Title: " Title "}, want: Citation{URI: "https://a.example/x", Title: "Title"}, ok: true},
		{name: "control chars in title rewritten", in: Citation{URI: "https://a.example", Title: "bad\x1b[31m<b>"}, want: Citation{URI: "https://a.example", Title: "bad_[31m_b_"}, ok: true},
		{name: "unicode control and bidi in title rewritten", in: Citation{URI: "https://a.example", Title: "a\u0085b\u202ec\u2066d\u200f"}, want: Citation{URI: "https://a.example", Title: "a_b_c_d_"}, ok: true},
		{name: "zero width joiner kept", in: Citation{URI: "https://a.example", Title: "\U0001F469\u200d\U0001F4BB"}, want: Citation{URI: "https://a.example", Title: "\U0001F469\u200d\U0001F4BB"}, ok: true},
		{name: "title capped", in: Citation{URI: "https://a.example", Title: strings.Repeat("é", 200)}, want: Citation{URI: "https://a.example", Title: strings.Repeat("é", 64)}, ok: true},
		{name: "empty uri", in: Citation{Title: "x"}},
		{name: "whitespace in uri", in: Citation{URI: "https://a.example/a b"}},
		{name: "unicode whitespace in uri", in: Citation{URI: "https://a.example/a\u00a0b"}},
		{name: "control char in uri", in: Citation{URI: "https://a.example/\x07"}},
		{name: "bidi override in uri", in: Citation{URI: "https://a.example/\u202e"}},
		{name: "c1 control in uri", in: Citation{URI: "https://a.example/\u009b"}},
		{name: "angle bracket in uri", in: Citation{URI: "https://a.example/<x>"}},
		{name: "overlong uri", in: Citation{URI: "https://a.example/" + strings.Repeat("x", maxCitationURIBytes)}},
	}
	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			t.Parallel()
			got, ok := SanitizeCitation(tt.in)
			assert.Equal(t, tt.ok, ok)
			assert.Equal(t, tt.want, got)
		})
	}
}
