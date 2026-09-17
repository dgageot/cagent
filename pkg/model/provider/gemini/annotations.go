package gemini

import (
	"encoding/json"
	"fmt"
	"slices"
	"strings"

	"google.golang.org/genai"

	"github.com/docker/docker-agent/pkg/chat"
)

const (
	serverToolCodeExecution = "code_execution"
	serverToolGoogleSearch  = "google_search"
	serverToolURLContext    = "url_context"
	serverToolUnknown       = "server_tool"

	partKindCode = "code"
	partKindTool = "tool"
)

// pendingCall is a server-side invocation whose result has not streamed yet.
// key is kind/candidate/id when the provider supplied an ID (see partKey);
// ID-less invocations pair by candidate and tool name instead.
type pendingCall struct {
	key       string
	candidate int32
	call      chat.ServerToolCall
}

// annotationTracker turns Gemini's response-side metadata into
// provider-neutral chat annotations: grounding chunks and citation metadata
// become citations; ExecutableCode/CodeExecutionResult and
// ToolCall/ToolResponse parts become complete server tool calls; web search
// queries and url_context statuses fill in when no explicit invocation parts
// were returned. Gemini repeats metadata and parts across streamed chunks,
// so the tracker keeps per-stream state. Only Recv touches it, sequentially.
type annotationTracker struct {
	seenURIs    map[string]struct{}
	seenQueries map[string]struct{}
	seenURLs    map[string]struct{}
	// seenParts holds "call/"+key and "result/"+key for ID-bearing parts so
	// a part repeated on a later chunk is not reported twice.
	seenParts map[string]struct{}
	// invoked lists tool names seen as explicit ToolCall parts; the metadata
	// summaries for those tools would duplicate them and are skipped.
	invoked map[string]struct{}
	pending []pendingCall
}

// hasAnnotations reports whether resp carries metadata the tracker can
// surface, so chunks with no text, media or function calls are forwarded.
func hasAnnotations(resp *genai.GenerateContentResponse) bool {
	for _, candidate := range resp.Candidates {
		if candidate == nil {
			continue
		}
		if gm := candidate.GroundingMetadata; gm != nil && (len(gm.GroundingChunks) > 0 || len(gm.WebSearchQueries) > 0) {
			return true
		}
		if cm := candidate.CitationMetadata; cm != nil && len(cm.Citations) > 0 {
			return true
		}
		if um := candidate.URLContextMetadata; um != nil && len(um.URLMetadata) > 0 {
			return true
		}
		if candidate.Content == nil {
			continue
		}
		for _, part := range candidate.Content.Parts {
			if part != nil && (part.ExecutableCode != nil || part.CodeExecutionResult != nil || part.ToolCall != nil || part.ToolResponse != nil) {
				return true
			}
		}
	}
	return false
}

// collect extracts the new citations and completed server tool calls from
// one streamed chunk. Invocations are held until their result arrives (or
// until flush), so every returned ServerToolCall is complete.
func (t *annotationTracker) collect(resp *genai.GenerateContentResponse) (citations []chat.Citation, calls []chat.ServerToolCall) {
	if t.seenURIs == nil {
		t.seenURIs = make(map[string]struct{})
		t.seenQueries = make(map[string]struct{})
		t.seenURLs = make(map[string]struct{})
		t.seenParts = make(map[string]struct{})
		t.invoked = make(map[string]struct{})
	}
	for _, candidate := range resp.Candidates {
		if candidate == nil {
			continue
		}
		if candidate.Content != nil {
			for _, part := range candidate.Content.Parts {
				calls = t.collectPart(calls, part, candidate.Index)
			}
		}
		if gm := candidate.GroundingMetadata; gm != nil {
			if _, explicit := t.invoked[serverToolGoogleSearch]; !explicit {
				for _, query := range gm.WebSearchQueries {
					if query != "" && markSeen(t.seenQueries, query) {
						calls = append(calls, chat.ServerToolCall{Name: serverToolGoogleSearch, Input: query})
					}
				}
			}
			for _, chunk := range gm.GroundingChunks {
				citations = t.appendCitation(citations, groundingChunkSource(chunk))
			}
		}
		if cm := candidate.CitationMetadata; cm != nil {
			for _, c := range cm.Citations {
				if c != nil {
					citations = t.appendCitation(citations, chat.Citation{URI: c.URI, Title: c.Title})
				}
			}
		}
		if um := candidate.URLContextMetadata; um != nil {
			if _, explicit := t.invoked[serverToolURLContext]; !explicit {
				for _, u := range um.URLMetadata {
					if u != nil && u.RetrievedURL != "" && markSeen(t.seenURLs, u.RetrievedURL) {
						calls = append(calls, urlRetrievalCall(u))
					}
				}
			}
		}
	}
	return citations, calls
}

func (t *annotationTracker) collectPart(calls []chat.ServerToolCall, part *genai.Part, candidate int32) []chat.ServerToolCall {
	if part == nil {
		return calls
	}
	if code := part.ExecutableCode; code != nil {
		t.addPending(partKey(partKindCode, candidate, code.ID), candidate, chat.ServerToolCall{
			Name:     serverToolCodeExecution,
			Language: strings.ToLower(string(code.Language)),
			Input:    code.Code,
		})
	}
	if result := part.CodeExecutionResult; result != nil {
		if call, ok := t.take(serverToolCodeExecution, partKey(partKindCode, candidate, result.ID), candidate); ok {
			call.Output = result.Output
			call.IsError = result.Outcome != "" && result.Outcome != genai.OutcomeOK
			calls = append(calls, call)
		}
	}
	if tc := part.ToolCall; tc != nil {
		name := serverToolName(tc.ToolType)
		t.invoked[name] = struct{}{}
		t.addPending(partKey(partKindTool, candidate, tc.ID), candidate, chat.ServerToolCall{
			Name:  name,
			Input: jsonOrEmpty(tc.Args),
		})
	}
	if tr := part.ToolResponse; tr != nil {
		if call, ok := t.take(serverToolName(tr.ToolType), partKey(partKindTool, candidate, tr.ID), candidate); ok {
			call.Output = jsonOrEmpty(tr.Response)
			calls = append(calls, call)
		}
	}
	return calls
}

// addPending queues an invocation, dropping ID-bearing repeats. ID-less
// invocations are always queued: the same code can legitimately run twice.
func (t *annotationTracker) addPending(key string, candidate int32, call chat.ServerToolCall) {
	if key != "" && !markSeen(t.seenParts, "call/"+key) {
		return
	}
	t.pending = append(t.pending, pendingCall{key: key, candidate: candidate, call: call})
}

// take pairs a result with its pending invocation: by key when the provider
// supplied an ID, otherwise the most recent ID-less invocation of the same
// tool in the same candidate (a result follows its code). An unpaired result
// stands alone under name. ok is false for a repeated ID-bearing result.
func (t *annotationTracker) take(name, key string, candidate int32) (call chat.ServerToolCall, ok bool) {
	if key != "" && !markSeen(t.seenParts, "result/"+key) {
		return chat.ServerToolCall{}, false
	}
	for i, p := range slices.Backward(t.pending) {
		matched := p.key == key
		if key == "" {
			matched = matched && p.candidate == candidate && p.call.Name == name
		}
		if !matched {
			continue
		}
		t.pending = slices.Delete(t.pending, i, i+1)
		return p.call, true
	}
	return chat.ServerToolCall{Name: name}, true
}

// flush returns invocations that never received a result, so a stream ending
// mid-execution still shows what the provider ran.
func (t *annotationTracker) flush() []chat.ServerToolCall {
	if len(t.pending) == 0 {
		return nil
	}
	calls := make([]chat.ServerToolCall, 0, len(t.pending))
	for _, p := range t.pending {
		calls = append(calls, p.call)
	}
	t.pending = nil
	return calls
}

func (t *annotationTracker) appendCitation(citations []chat.Citation, c chat.Citation) []chat.Citation {
	if c.URI == "" || !markSeen(t.seenURIs, c.URI) {
		return citations
	}
	return append(citations, c)
}

// partKey scopes a provider-supplied part ID by kind and candidate so code
// and tool IDs cannot collide and parallel candidates cannot pair with each
// other. Empty when the provider gave no ID.
func partKey(kind string, candidate int32, id string) string {
	if id == "" {
		return ""
	}
	return fmt.Sprintf("%s/%d/%s", kind, candidate, id)
}

// markSeen records key in set and reports whether it was new.
func markSeen(set map[string]struct{}, key string) bool {
	if _, dup := set[key]; dup {
		return false
	}
	set[key] = struct{}{}
	return true
}

// serverToolName maps a Gemini ToolType to the provider_opts tool name users
// configured, so explicit invocations and metadata summaries share a name.
func serverToolName(tt genai.ToolType) string {
	switch tt {
	case genai.ToolTypeGoogleSearchWeb:
		return serverToolGoogleSearch
	case "", genai.ToolTypeUnspecified:
		return serverToolUnknown
	}
	return strings.ToLower(string(tt))
}

func jsonOrEmpty(v map[string]any) string {
	if len(v) == 0 {
		return ""
	}
	encoded, err := json.Marshal(v)
	if err != nil {
		return ""
	}
	return string(encoded)
}

// urlRetrievalCall reports one url_context fetch with its status, marking
// blocked or failed retrievals as errors.
func urlRetrievalCall(u *genai.URLMetadata) chat.ServerToolCall {
	status := u.URLRetrievalStatus
	return chat.ServerToolCall{
		Name:    serverToolURLContext,
		Input:   u.RetrievedURL,
		Output:  strings.ToLower(strings.TrimPrefix(string(status), "URL_RETRIEVAL_STATUS_")),
		IsError: status != "" && status != genai.URLRetrievalStatusUnspecified && status != genai.URLRetrievalStatusSuccess,
	}
}

// groundingChunkSource maps the populated variant of a grounding chunk to a
// citation. Chunks without a URI yield an empty citation that callers drop.
func groundingChunkSource(chunk *genai.GroundingChunk) chat.Citation {
	switch {
	case chunk == nil:
		return chat.Citation{}
	case chunk.Web != nil:
		return chat.Citation{URI: chunk.Web.URI, Title: chunk.Web.Title}
	case chunk.RetrievedContext != nil:
		return chat.Citation{URI: chunk.RetrievedContext.URI, Title: chunk.RetrievedContext.Title}
	case chunk.Maps != nil:
		return chat.Citation{URI: chunk.Maps.URI, Title: chunk.Maps.Title}
	}
	return chat.Citation{}
}
