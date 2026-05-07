package root

import (
	"context"
	"fmt"
	"net"
	"os"
	"runtime"
	"strings"
	"time"

	"github.com/google/uuid"
	"go.opentelemetry.io/otel"
	"go.opentelemetry.io/otel/attribute"
	"go.opentelemetry.io/otel/exporters/otlp/otlplog/otlploghttp"
	"go.opentelemetry.io/otel/exporters/otlp/otlpmetric/otlpmetrichttp"
	"go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
	"go.opentelemetry.io/otel/log/global"
	"go.opentelemetry.io/otel/propagation"
	"go.opentelemetry.io/otel/sdk/log"
	"go.opentelemetry.io/otel/sdk/metric"
	"go.opentelemetry.io/otel/sdk/resource"
	"go.opentelemetry.io/otel/sdk/trace"
	semconv "go.opentelemetry.io/otel/semconv/v1.40.0"

	"github.com/docker/docker-agent/pkg/httpclient"
	"github.com/docker/docker-agent/pkg/version"
)

const AppName = "cagent"

// initOTelSDK initializes OpenTelemetry SDK with OTLP exporter
func initOTelSDK(ctx context.Context) (err error) {
	res, err := newOTelResource()
	if err != nil {
		return fmt.Errorf("failed to create resource: %w", err)
	}

	endpoint := os.Getenv("OTEL_EXPORTER_OTLP_ENDPOINT")

	tp, err := newTracerProvider(ctx, res, endpoint)
	if err != nil {
		return fmt.Errorf("failed to create tracer provider: %w", err)
	}
	otel.SetTracerProvider(tp)

	mp, err := newMeterProvider(ctx, res, endpoint)
	if err != nil {
		_ = shutdownTracerProvider(tp)
		return fmt.Errorf("failed to create meter provider: %w", err)
	}
	otel.SetMeterProvider(mp)

	lp, err := newLoggerProvider(ctx, res, endpoint)
	if err != nil {
		_ = mp.Shutdown(context.Background())
		_ = shutdownTracerProvider(tp)
		return fmt.Errorf("failed to create logger provider: %w", err)
	}
	global.SetLoggerProvider(lp)

	// W3C trace context + baggage propagation across processes.
	otel.SetTextMapPropagator(propagation.NewCompositeTextMapPropagator(
		propagation.TraceContext{},
		propagation.Baggage{},
	))

	httpclient.SetOTelEnabled(true)

	go func() {
		<-ctx.Done()
		// Flush logs and metrics before traces; give each its own budget.
		shutdown := func(fn func(context.Context) error) {
			c, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			_ = fn(c)
		}
		shutdown(lp.Shutdown)
		shutdown(mp.Shutdown)
		shutdown(tp.Shutdown)
	}()

	return nil
}

func newTracerProvider(ctx context.Context, res *resource.Resource, endpoint string) (*trace.TracerProvider, error) {
	opts := []trace.TracerProviderOption{trace.WithResource(res)}

	if endpoint == "" {
		return trace.NewTracerProvider(opts...), nil
	}

	exp, err := otlptracehttp.New(ctx, traceExporterOptions(endpoint)...)
	if err != nil {
		return nil, fmt.Errorf("failed to create trace exporter: %w", err)
	}
	opts = append(opts, trace.WithBatcher(exp,
		trace.WithBatchTimeout(5*time.Second),
		trace.WithMaxExportBatchSize(512),
	))
	return trace.NewTracerProvider(opts...), nil
}

func newMeterProvider(ctx context.Context, res *resource.Resource, endpoint string) (*metric.MeterProvider, error) {
	opts := []metric.Option{metric.WithResource(res)}

	if endpoint != "" {
		exp, err := otlpmetrichttp.New(ctx, metricExporterOptions(endpoint)...)
		if err != nil {
			return nil, fmt.Errorf("failed to create metric exporter: %w", err)
		}
		opts = append(opts, metric.WithReader(metric.NewPeriodicReader(exp,
			metric.WithInterval(60*time.Second),
		)))
	}

	return metric.NewMeterProvider(opts...), nil
}

func newLoggerProvider(ctx context.Context, res *resource.Resource, endpoint string) (*log.LoggerProvider, error) {
	opts := []log.LoggerProviderOption{log.WithResource(res)}

	if endpoint != "" {
		exp, err := otlploghttp.New(ctx, logExporterOptions(endpoint)...)
		if err != nil {
			return nil, fmt.Errorf("failed to create log exporter: %w", err)
		}
		opts = append(opts, log.WithProcessor(log.NewBatchProcessor(exp)))
	}

	return log.NewLoggerProvider(opts...), nil
}

// normalizeOTLPEndpoint pins a scheme on bare host:port endpoints so
// the trace, metric, and log OTLP/HTTP exporters agree on transport.
func normalizeOTLPEndpoint(endpoint string) string {
	if strings.HasPrefix(endpoint, "http://") || strings.HasPrefix(endpoint, "https://") {
		return endpoint
	}
	if isLocalhostEndpoint(endpoint) {
		return "http://" + endpoint
	}
	return "https://" + endpoint
}

func traceExporterOptions(endpoint string) []otlptracehttp.Option {
	return []otlptracehttp.Option{otlptracehttp.WithEndpointURL(normalizeOTLPEndpoint(endpoint))}
}

func metricExporterOptions(endpoint string) []otlpmetrichttp.Option {
	return []otlpmetrichttp.Option{otlpmetrichttp.WithEndpointURL(normalizeOTLPEndpoint(endpoint))}
}

func logExporterOptions(endpoint string) []otlploghttp.Option {
	return []otlploghttp.Option{otlploghttp.WithEndpointURL(normalizeOTLPEndpoint(endpoint))}
}

func shutdownTracerProvider(tp *trace.TracerProvider) error {
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return tp.Shutdown(shutdownCtx)
}

func newOTelResource() (*resource.Resource, error) {
	attrs := []attribute.KeyValue{
		semconv.ServiceName(AppName),
		semconv.ServiceVersion(version.Version),
		semconv.ServiceInstanceID(uuid.NewString()),
		semconv.ProcessPID(os.Getpid()),
		semconv.ProcessRuntimeName("go"),
		semconv.OSTypeKey.String(runtime.GOOS),
		semconv.HostArchKey.String(runtime.GOARCH),
	}
	if hostname, err := os.Hostname(); err == nil && hostname != "" {
		attrs = append(attrs, semconv.HostName(hostname))
	}
	return resource.Merge(
		resource.Default(),
		resource.NewWithAttributes(semconv.SchemaURL, attrs...),
	)
}

// isLocalhostEndpoint reports whether the given endpoint refers to a
// loopback address so that we can safely skip TLS.
func isLocalhostEndpoint(endpoint string) bool {
	host := endpoint
	// Strip port if present.
	if h, _, err := net.SplitHostPort(endpoint); err == nil {
		host = h
	}
	// Strip brackets from IPv6 addresses (e.g. "[::1]" without a port).
	host = strings.TrimPrefix(host, "[")
	host = strings.TrimSuffix(host, "]")
	return host == "localhost" || host == "127.0.0.1" || host == "::1"
}
