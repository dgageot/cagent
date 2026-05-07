package oaistream

import (
	"encoding/base64"
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/attachment/modelcaps"
	"github.com/docker/docker-agent/pkg/chat"
)

// minJPEG is a minimal JPEG magic-byte header for use in tests.
var minJPEG = []byte{0xFF, 0xD8, 0xFF, 0xE0}

// TestConvertDocument_StrategyB64_Image verifies that an image document with
// InlineData and a vision-capable model produces an image content part with
// a data-URI, not a text part.
func TestConvertDocument_StrategyB64_Image(t *testing.T) {
	doc := chat.Document{
		Name:     "photo.jpg",
		MimeType: "image/jpeg",
		Source:   chat.DocumentSource{InlineData: minJPEG},
	}

	visionCaps := modelcaps.CapsWith(true, true)
	parts, err := convertDocumentWithCaps(t.Context(), doc, visionCaps)
	require.NoError(t, err)
	require.Len(t, parts, 1, "expected exactly one image part")
	require.NotNil(t, parts[0].OfImageURL, "expected image part, got non-image")
	assert.Nil(t, parts[0].OfText, "expected no text part for B64 image")

	// Data URI must embed the base64-encoded payload.
	wantB64 := base64.StdEncoding.EncodeToString(minJPEG)
	assert.Contains(t, parts[0].OfImageURL.ImageURL.URL, "data:image/jpeg;base64,")
	assert.Contains(t, parts[0].OfImageURL.ImageURL.URL, wantB64)
}

// TestConvertDocument_StrategyB64_ImageDropped verifies that an image is
// dropped when the model does not support vision.
func TestConvertDocument_StrategyB64_ImageDropped(t *testing.T) {
	doc := chat.Document{
		Name:     "photo.jpg",
		MimeType: "image/jpeg",
		Source:   chat.DocumentSource{InlineData: minJPEG},
	}

	textOnlyCaps := modelcaps.CapsWith(false, false)
	parts, err := convertDocumentWithCaps(t.Context(), doc, textOnlyCaps)
	require.NoError(t, err)
	assert.Nil(t, parts, "image should be dropped for text-only model")
}

func TestConvertDocument_StrategyTXT(t *testing.T) {
	doc := chat.Document{
		Name:     "readme.md",
		MimeType: "text/markdown",
		Source:   chat.DocumentSource{InlineText: "# Hello World"},
	}

	parts, err := convertDocument(t.Context(), doc, "")
	require.NoError(t, err)
	require.Len(t, parts, 1)
	require.NotNil(t, parts[0].OfText)
	assert.Contains(t, parts[0].OfText.Text, "readme-md")
	assert.Contains(t, parts[0].OfText.Text, "text-markdown")
	assert.Contains(t, parts[0].OfText.Text, "# Hello World")
}

func TestConvertDocument_StrategyTXT_Envelope(t *testing.T) {
	doc := chat.Document{
		Name:     "data.csv",
		MimeType: "text/csv",
		Source:   chat.DocumentSource{InlineText: "a,b,c\n1,2,3"},
	}

	parts, err := convertDocument(t.Context(), doc, "")
	require.NoError(t, err)
	require.Len(t, parts, 1)
	require.NotNil(t, parts[0].OfText)
	text := parts[0].OfText.Text
	assert.True(t, strings.HasPrefix(text, "<document"), "should be wrapped in document envelope")
}

func TestConvertDocument_Drop_NoContent(t *testing.T) {
	doc := chat.Document{
		Name:     "empty.txt",
		MimeType: "text/plain",
		Source:   chat.DocumentSource{},
	}

	parts, err := convertDocument(t.Context(), doc, "")
	require.NoError(t, err)
	assert.Nil(t, parts, "should be dropped when no inline content")
}
