package mcp

// MCP attribute keys defined by the OTel semantic conventions.
const (
	AttrMethodName      = "mcp.method.name"
	AttrProtocolVersion = "mcp.protocol.version"
	AttrResourceURI     = "mcp.resource.uri"
	AttrSessionID       = "mcp.session.id"
)

// JSON-RPC attribute keys used alongside MCP spans.
const (
	AttrJSONRPCRequestID       = "jsonrpc.request.id"
	AttrJSONRPCProtocolVersion = "jsonrpc.protocol.version"
	AttrRPCResponseStatusCode  = "rpc.response.status_code"
)

// gen_ai.* attribute keys overlaid on MCP spans (duplicated as constants here
// to avoid depending on the genai package).
const (
	AttrGenAIOperationName = "gen_ai.operation.name"
	AttrGenAIToolName      = "gen_ai.tool.name"
	AttrGenAIPromptName    = "gen_ai.prompt.name"
)

// Well-known MCP method names.
const (
	MethodInitialize         = "initialize"
	MethodPing               = "ping"
	MethodCompletionComplete = "completion/complete"
	MethodPromptsList        = "prompts/list"
	MethodPromptsGet         = "prompts/get"
	MethodResourcesList      = "resources/list"
	MethodResourcesRead      = "resources/read"
	MethodResourcesSubscribe = "resources/subscribe"
	MethodResourcesUnsub     = "resources/unsubscribe"
	MethodResourcesTemplates = "resources/templates/list"
	MethodRootsList          = "roots/list"
	MethodSamplingCreate     = "sampling/createMessage"
	MethodToolsList          = "tools/list"
	MethodToolsCall          = "tools/call"
	MethodLoggingSetLevel    = "logging/setLevel"
	MethodElicitationCreate  = "elicitation/create"
)

// OperationExecuteTool is the gen_ai.operation.name value for tools/call.
const OperationExecuteTool = "execute_tool"

// instrumentationName is the OTel instrumentation scope used by this package.
const instrumentationName = "github.com/docker/docker-agent/pkg/telemetry/mcp"
