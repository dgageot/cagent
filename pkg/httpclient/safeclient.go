package httpclient

import (
	"net/http"
	"time"
)

// NewSafeClient returns the HTTP client used by built-in tools that issue
// outbound calls to URLs the operator (or a fetched OpenAPI spec) supplies.
//
// The default refuses connections to non-public IPs at dial time
// — defeating DNS rebinding to loopback / RFC1918 / link-local incl. cloud
// metadata at 169.254.169.254 — and bounds the redirect chain at 10 hops.
// The transport is wrapped with [WrapWithOTel] so outbound calls inject
// W3C `traceparent` and emit HTTP CLIENT spans when OTel is enabled; the
// wrap is a no-op otherwise.
//
// When unsafe is true the client uses [http.DefaultTransport]. This branch
// exists ONLY for tests, which use [httptest.NewServer] (binds to 127.0.0.1)
// and therefore cannot pass the SSRF check.
func NewSafeClient(timeout time.Duration, unsafe bool) *http.Client {
	if unsafe {
		return &http.Client{Timeout: timeout, Transport: WrapWithOTel(http.DefaultTransport)}
	}
	return &http.Client{
		Timeout:       timeout,
		Transport:     WrapWithOTel(NewSSRFSafeTransport()),
		CheckRedirect: BoundedRedirects(10),
	}
}
