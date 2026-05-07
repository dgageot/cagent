package genai

import (
	"os"
	"strings"
	"sync"

	"go.opentelemetry.io/otel/attribute"
)

// EnvSemconvStability is the OTel-defined env var that opts into
// experimental versions of the GenAI semantic conventions.
const EnvSemconvStability = "OTEL_SEMCONV_STABILITY_OPT_IN"

// stabilityToken is the spec opt-in for the latest experimental conventions.
const stabilityToken = "gen_ai_latest_experimental"

// Stability identifies which version of attribute names a span should emit.
type Stability int

const (
	// StabilityDualEmit emits both legacy and gen_ai.* keys.
	StabilityDualEmit Stability = iota
	// StabilityGenAILatest emits only gen_ai.* keys.
	StabilityGenAILatest
)

var (
	stabilityMu     sync.Mutex
	stabilityOnce   sync.Once
	cachedStability Stability
)

// CurrentStability returns the active stability mode, computed once per process.
func CurrentStability() Stability {
	stabilityMu.Lock()
	once := &stabilityOnce
	stabilityMu.Unlock()

	once.Do(func() {
		raw := os.Getenv(EnvSemconvStability)
		for tok := range strings.SplitSeq(raw, ",") {
			if strings.EqualFold(strings.TrimSpace(tok), stabilityToken) {
				stabilityMu.Lock()
				cachedStability = StabilityGenAILatest
				stabilityMu.Unlock()
				return
			}
		}
		stabilityMu.Lock()
		cachedStability = StabilityDualEmit
		stabilityMu.Unlock()
	})

	stabilityMu.Lock()
	defer stabilityMu.Unlock()
	return cachedStability
}

// ResetStabilityForTest clears the cached stability value. Test-only;
// callers must run sequentially (no t.Parallel).
func ResetStabilityForTest() {
	stabilityMu.Lock()
	defer stabilityMu.Unlock()
	stabilityOnce = sync.Once{}
	cachedStability = StabilityDualEmit
}

// EmitLegacyAttributes reports whether legacy attribute keys should be emitted.
func EmitLegacyAttributes() bool {
	return CurrentStability() == StabilityDualEmit
}

// LegacyToolAttributes returns the historic tool dispatcher attribute set,
// or nil when legacy emission is disabled.
func LegacyToolAttributes(toolName, toolType, agentName, sessionID, callID string) []attribute.KeyValue {
	if !EmitLegacyAttributes() {
		return nil
	}
	attrs := []attribute.KeyValue{
		attribute.String("tool.name", toolName),
		attribute.String("agent", agentName),
		attribute.String("session.id", sessionID),
	}
	if toolType != "" {
		attrs = append(attrs, attribute.String("tool.type", toolType))
	}
	if callID != "" {
		attrs = append(attrs, attribute.String("tool.call_id", callID))
	}
	return attrs
}
