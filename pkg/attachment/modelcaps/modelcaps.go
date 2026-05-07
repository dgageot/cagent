// Package modelcaps provides model capability queries for the attachment system.
// It translates models.dev modality information into MIME-type support decisions
// used by the attachment routing logic.
package modelcaps

import (
	"context"
	"log/slog"
	"strings"
	"time"

	"github.com/docker/docker-agent/pkg/modelsdev"
)

// ModelCapabilities describes what MIME types a given model can accept as
// document attachments.
type ModelCapabilities struct {
	// supportsImage is true when the model accepts image/* MIME types.
	supportsImage bool
	// supportsPDF is true when the model accepts application/pdf.
	supportsPDF bool
	// modelFound is false when models.dev has no record for this model,
	// which causes conservative fallback behaviour (text-only).
	modelFound bool
}

// isOfficeMIME returns true for Office document binary formats
// (OOXML, legacy Office, RTF). These are ZIP-based or binary formats
// that cannot be naively TXT-enveloped and require explicit model support.
func isOfficeMIME(mt string) bool {
	switch mt {
	case "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
		"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
		"application/vnd.openxmlformats-officedocument.presentationml.presentation",
		"application/vnd.ms-excel",
		"application/vnd.ms-powerpoint",
		"application/msword",
		"application/rtf",
		"text/rtf":
		return true
	}
	return false
}

// Supports returns true when the model can accept an attachment with the given
// MIME type.
//
// Resolution rules (in order):
//  1. image/* → requires supportsImage (models.dev "image" modality)
//  2. application/pdf → requires supportsPDF (models.dev "pdf" modality)
//  3. text/* → always supported (plain text; TXT envelope is universally safe)
//  4. Office/binary document MIMEs (DOCX, XLSX, PPTX, etc.) → not supported unless
//     models.dev explicitly declares a document modality. models.dev currently has
//     no "document" or "office" modality field, so these return false for all
//     models until the schema is extended.
//  5. Everything else (audio/*, video/*, unknown binary) → false
func (mc ModelCapabilities) Supports(mimeType string) bool {
	mt := strings.ToLower(mimeType)
	if strings.HasPrefix(mt, "image/") {
		return mc.supportsImage
	}
	if mt == "application/pdf" {
		return mc.supportsPDF
	}
	// text/* MIMEs (text/plain, text/markdown, text/html, text/csv, …) are always
	// supported — they are actual text and TXT envelope works universally.
	if strings.HasPrefix(mt, "text/") {
		return true
	}
	// Office document formats (DOCX, XLSX, PPTX, etc.) are ZIP-based binaries;
	// they cannot be naively TXT-enveloped. models.dev does not yet declare an
	// "office" or "document" modality, so we conservatively return false until
	// the schema provides explicit capability data.
	if isOfficeMIME(mt) {
		return false
	}
	// audio/*, video/*, and all other unknown binary types are not supported.
	return false
}

// loadTimeout is the maximum time allowed for a models.dev capability lookup.
// If the fetch takes longer, Load falls back to conservative text-only caps.
const loadTimeout = 10 * time.Second

// Load fetches (or returns from cache) the capability record for the given
// model ID.  The model ID should be in "provider/model" format as used by
// models.dev (e.g. "anthropic/claude-3-5-sonnet-20241022").
//
// When the model is not found in the models.dev database, Load returns a
// conservative capability set that only allows text MIME types.  The returned
// error is always nil; capability detection failures are silent and safe.
func Load(modelID string) (ModelCapabilities, error) {
	ctx, cancel := context.WithTimeout(context.Background(), loadTimeout)
	defer cancel()

	store, err := modelsdev.NewStore()
	if err != nil {
		slog.WarnContext(ctx, "modelcaps: failed to load models.dev store, using conservative caps",
			"error", err, "model", modelID)
		return ModelCapabilities{modelFound: false}, nil
	}

	model, err := store.GetModel(ctx, modelID)
	if err != nil {
		if ctx.Err() != nil {
			slog.WarnContext(ctx, "modelcaps: models.dev lookup timed out, using conservative caps",
				"model", modelID, "timeout", loadTimeout)
		}
		// Model not found or context cancelled — conservative: text-only.
		return ModelCapabilities{modelFound: false}, nil
	}

	mc := ModelCapabilities{modelFound: true}
	for _, input := range model.Modalities.Input {
		switch strings.ToLower(input) {
		case "image":
			mc.supportsImage = true
		case "pdf":
			mc.supportsPDF = true
		}
	}
	return mc, nil
}

// CapsWith constructs a ModelCapabilities value directly from booleans. This is
// intended for use in tests and provider implementations that need to create a
// capabilities value without hitting the network.
func CapsWith(supportsImage, supportsPDF bool) ModelCapabilities {
	return ModelCapabilities{
		supportsImage: supportsImage,
		supportsPDF:   supportsPDF,
		modelFound:    true,
	}
}

// LoadFromStore is like Load but accepts an explicit *modelsdev.Store, making
// it convenient for tests that inject a pre-populated in-memory store.
func LoadFromStore(store *modelsdev.Store, modelID string) ModelCapabilities {
	ctx, cancel := context.WithTimeout(context.Background(), loadTimeout)
	defer cancel()

	model, err := store.GetModel(ctx, modelID)
	if err != nil {
		return ModelCapabilities{modelFound: false}
	}

	mc := ModelCapabilities{modelFound: true}
	for _, input := range model.Modalities.Input {
		switch strings.ToLower(input) {
		case "image":
			mc.supportsImage = true
		case "pdf":
			mc.supportsPDF = true
		}
	}
	return mc
}
