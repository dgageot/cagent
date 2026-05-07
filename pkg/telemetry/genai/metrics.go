package genai

import (
	"sync"

	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/metric"
)

// instrumentationName is the OTel instrumentation scope used by this package.
const instrumentationName = "github.com/docker/docker-agent/pkg/telemetry/genai"

// metricBucketsDuration matches the spec for `gen_ai.client.operation.duration`.
var metricBucketsDuration = []float64{
	0.01, 0.02, 0.04, 0.08, 0.16, 0.32, 0.64, 1.28, 2.56, 5.12, 10.24, 20.48, 40.96, 81.92,
}

// metricBucketsTokenUsage matches the spec for `gen_ai.client.token.usage`.
var metricBucketsTokenUsage = []float64{
	1, 4, 16, 64, 256, 1024, 4096, 16384, 65536, 262144, 1048576, 4194304, 16777216, 67108864,
}

// instruments holds the lazily-initialised metric instruments. Resolved on
// first use because the global MeterProvider is set at SDK init time.
type instruments struct {
	clientOperationDuration     metric.Float64Histogram
	clientOperationTTFC         metric.Float64Histogram
	clientOperationTimePerChunk metric.Float64Histogram
	clientTokenUsage            metric.Int64Histogram
}

var (
	instOnce sync.Once
	inst     *instruments
)

// getInstruments resolves and caches the package-level meter instruments.
// Instruments are bound to the global MeterProvider on first call (sync.Once)
// and not rebound by later otel.SetMeterProvider calls; tests must install
// their provider before any instrumented code path runs.
func getInstruments() *instruments {
	instOnce.Do(func() {
		meter := otel.Meter(instrumentationName)
		i := &instruments{}

		i.clientOperationDuration, _ = meter.Float64Histogram(
			"gen_ai.client.operation.duration",
			metric.WithUnit("s"),
			metric.WithDescription("GenAI operation duration."),
			metric.WithExplicitBucketBoundaries(metricBucketsDuration...),
		)
		i.clientOperationTTFC, _ = meter.Float64Histogram(
			"gen_ai.client.operation.time_to_first_chunk",
			metric.WithUnit("s"),
			metric.WithDescription("Time to receive the first chunk of a streaming GenAI response."),
			metric.WithExplicitBucketBoundaries(metricBucketsDuration...),
		)
		i.clientOperationTimePerChunk, _ = meter.Float64Histogram(
			"gen_ai.client.operation.time_per_output_chunk",
			metric.WithUnit("s"),
			metric.WithDescription("Time between consecutive output chunks of a streaming GenAI response."),
			metric.WithExplicitBucketBoundaries(metricBucketsDuration...),
		)
		i.clientTokenUsage, _ = meter.Int64Histogram(
			"gen_ai.client.token.usage",
			metric.WithUnit("{token}"),
			metric.WithDescription("Number of tokens used in a GenAI client request, broken down by token type."),
			metric.WithExplicitBucketBoundaries(metricBucketsTokenUsage...),
		)

		inst = i
	})
	return inst
}
