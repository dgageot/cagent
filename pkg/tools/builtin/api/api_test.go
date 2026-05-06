package api

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/js"
	"github.com/docker/docker-agent/pkg/tools"
)

type testServer struct {
	serverURL       string
	receivedURL     string
	receivedHeaders http.Header
	receivedMethod  string
	receivedBody    []byte
}

func getTestServer(t *testing.T) *testServer {
	t.Helper()

	ts := &testServer{}
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		ts.receivedHeaders = r.Header.Clone()
		ts.receivedURL = r.URL.String()
		ts.receivedMethod = r.Method

		var err error
		ts.receivedBody, err = io.ReadAll(r.Body)
		if err != nil {
			w.WriteHeader(http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte(`{"status":"ok"}`))
	}))

	ts.serverURL = server.URL

	t.Cleanup(func() {
		server.Close()
	})

	return ts
}

func TestAPITool_GET(t *testing.T) {
	t.Parallel()
	ts := getTestServer(t)

	tool := NewAPITool(latest.APIToolConfig{
		Method:   http.MethodGet,
		Endpoint: ts.serverURL + "/api?key=${key}&value=${value}",
	}, testExpander())

	result, err := tool.callTool(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"key": "mykey", "value": "myvalue"}`,
		},
	})

	require.NoError(t, err)
	assert.JSONEq(t, `{"status":"ok"}`, result.Output)
	assert.Equal(t, http.MethodGet, ts.receivedMethod)
	assert.Equal(t, "/api?key=mykey&value=myvalue", ts.receivedURL)
}

func TestAPITool_POST(t *testing.T) {
	t.Parallel()
	ts := getTestServer(t)

	tool := NewAPITool(latest.APIToolConfig{
		Method:   http.MethodPost,
		Endpoint: ts.serverURL,
	}, testExpander())

	result, err := tool.callTool(t.Context(), tools.ToolCall{
		Function: tools.FunctionCall{
			Arguments: `{"name":"John Doe","age":30}`,
		},
	})

	require.NoError(t, err)
	assert.JSONEq(t, `{"status":"ok"}`, result.Output)
	assert.Equal(t, http.MethodPost, ts.receivedMethod)
	assert.Equal(t, "application/json", ts.receivedHeaders.Get("Content-Type"))

	var receivedData map[string]any
	err = json.Unmarshal(ts.receivedBody, &receivedData)
	require.NoError(t, err)
	assert.Equal(t, "John Doe", receivedData["name"])
	assert.InEpsilon(t, 30.0, receivedData["age"], 0.01)
}

func TestAPITool_Headers(t *testing.T) {
	t.Parallel()
	ts := getTestServer(t)

	tool := NewAPITool(latest.APIToolConfig{
		Method:   http.MethodGet,
		Endpoint: ts.serverURL,
		Headers: map[string]string{
			"X-Custom-Header":  "custom-value",
			"X-API-Key":        "secret-key",
			"X-Another-Header": "another-value",
		},
	}, testExpander())

	result, err := tool.callTool(t.Context(), tools.ToolCall{})

	require.NoError(t, err)
	assert.JSONEq(t, `{"status":"ok"}`, result.Output)
	assert.Equal(t, "custom-value", ts.receivedHeaders.Get("X-Custom-Header"))
	assert.Equal(t, "secret-key", ts.receivedHeaders.Get("X-API-Key"))
	assert.Equal(t, "another-value", ts.receivedHeaders.Get("X-Another-Header"))
}

func TestAPITool_DefaultOutputSchema(t *testing.T) {
	t.Parallel()

	tool := NewAPITool(latest.APIToolConfig{
		Name:     "default-schema",
		Method:   http.MethodGet,
		Endpoint: "https://example.com/api",
	}, testExpander())

	toolsList, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, toolsList, 1)

	assert.Equal(t, tools.MustSchemaFor[string](), toolsList[0].OutputSchema)
}

func TestAPITool_CustomOutputSchema(t *testing.T) {
	t.Parallel()

	customSchema := map[string]any{
		"type": "object",
		"properties": map[string]any{
			"first_name": map[string]any{"type": "string"},
			"last_name":  map[string]any{"type": "string"},
			"age":        map[string]any{"type": "number"},
		},
		"required": []string{"first_name", "last_name"},
	}

	tool := NewAPITool(latest.APIToolConfig{
		Name:         "user-info",
		Method:       http.MethodGet,
		Endpoint:     "https://example.com/api/users/${id}",
		Required:     []string{"id"},
		Args:         map[string]any{"id": map[string]any{"type": "number"}},
		OutputSchema: customSchema,
	}, testExpander())

	toolsList, err := tool.Tools(t.Context())
	require.NoError(t, err)
	require.Len(t, toolsList, 1)

	schema, ok := toolsList[0].OutputSchema.(map[string]any)
	require.True(t, ok)
	assert.Equal(t, "object", schema["type"])

	props, ok := schema["properties"].(map[string]any)
	require.True(t, ok)
	assert.Contains(t, props, "first_name")
	assert.Contains(t, props, "age")
}

type noopEnvProvider struct{}

func (noopEnvProvider) Get(context.Context, string) (string, bool) { return "", false }

func testExpander() *js.Expander {
	return js.NewJsExpander(noopEnvProvider{})
}
