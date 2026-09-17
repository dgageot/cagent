package gemini

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	"google.golang.org/genai"

	"github.com/docker/docker-agent/pkg/chat"
	"github.com/docker/docker-agent/pkg/config/latest"
	"github.com/docker/docker-agent/pkg/environment"
	"github.com/docker/docker-agent/pkg/modelsdev"
)

func textChunk(text string) *genai.GenerateContentResponse {
	return &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{{Text: text}}},
	}}}
}

func groundedChunk(text string, queries []string, chunks ...*genai.GroundingChunk) *genai.GenerateContentResponse {
	resp := textChunk(text)
	resp.Candidates[0].GroundingMetadata = &genai.GroundingMetadata{
		WebSearchQueries: queries,
		GroundingChunks:  chunks,
	}
	return resp
}

func webChunk(uri, title string) *genai.GroundingChunk {
	return &genai.GroundingChunk{Web: &genai.GroundingChunkWeb{URI: uri, Title: title}}
}

func iterOf(chunks ...*genai.GenerateContentResponse) func(func(*genai.GenerateContentResponse, error) bool) {
	return func(fn func(*genai.GenerateContentResponse, error) bool) {
		for _, c := range chunks {
			if !fn(c, nil) {
				return
			}
		}
	}
}

// drain collects every delta until the terminal chunk, returning the deltas
// and the final response.
func drain(t *testing.T, adapter *StreamAdapter) ([]chat.MessageDelta, chat.MessageStreamResponse) {
	t.Helper()
	var deltas []chat.MessageDelta
	for {
		resp, err := adapter.Recv()
		require.NoError(t, err)
		require.Len(t, resp.Choices, 1)
		if resp.Choices[0].FinishReason != "" {
			_, err = adapter.Recv()
			require.ErrorIs(t, err, io.EOF)
			return deltas, resp
		}
		deltas = append(deltas, resp.Choices[0].Delta)
	}
}

func TestStreamAdapter_CodeExecutionPairsCodeWithResult(t *testing.T) {
	t.Parallel()

	codeChunk := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{{
			ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: "print(6*7)"},
		}}},
	}}}
	resultChunk := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{{
			CodeExecutionResult: &genai.CodeExecutionResult{Outcome: genai.OutcomeOK, Output: "42\n"},
		}}},
	}}}

	adapter := NewStreamAdapter(iterOf(codeChunk, resultChunk, textChunk("The answer is 42.")), "m", false)
	deltas, final := drain(t, adapter)

	require.Len(t, deltas, 3, "metadata-only chunks must be forwarded")
	assert.Empty(t, deltas[0].ServerToolCalls, "code is held until its result arrives")
	assert.Empty(t, deltas[0].Content)
	assert.Equal(t, []chat.ServerToolCall{{
		Name:     "code_execution",
		Language: "python",
		Input:    "print(6*7)",
		Output:   "42\n",
	}}, deltas[1].ServerToolCalls)
	assert.Equal(t, "The answer is 42.", deltas[2].Content)
	assert.Empty(t, deltas[2].ServerToolCalls)
	assert.Empty(t, final.Choices[0].Delta.ServerToolCalls)
	assert.Equal(t, chat.FinishReasonStop, final.Choices[0].FinishReason)
	for _, d := range deltas {
		assert.Empty(t, d.ToolCalls, "server-side execution must never surface as a local tool call")
	}
}

func TestStreamAdapter_CodeExecutionFailureAndSameChunk(t *testing.T) {
	t.Parallel()

	chunk := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{
			{ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: "1/0"}},
			{CodeExecutionResult: &genai.CodeExecutionResult{Outcome: genai.OutcomeFailed, Output: "ZeroDivisionError"}},
			{Text: "That failed."},
		}},
	}}}

	deltas, _ := drain(t, NewStreamAdapter(iterOf(chunk), "m", false))

	require.Len(t, deltas, 1)
	assert.Equal(t, "That failed.", deltas[0].Content)
	assert.Equal(t, []chat.ServerToolCall{{
		Name: "code_execution", Language: "python", Input: "1/0", Output: "ZeroDivisionError", IsError: true,
	}}, deltas[0].ServerToolCalls)
}

func TestStreamAdapter_CodeWithoutResultFlushedOnDone(t *testing.T) {
	t.Parallel()

	codeChunk := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{{
			ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: "while True: pass"},
		}}},
	}}}

	deltas, final := drain(t, NewStreamAdapter(iterOf(codeChunk), "m", false))

	require.Len(t, deltas, 1)
	assert.Empty(t, deltas[0].ServerToolCalls)
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "while True: pass"}},
		final.Choices[0].Delta.ServerToolCalls, "unfinished code stays visible on the terminal chunk")
	assert.Equal(t, chat.FinishReasonStop, final.Choices[0].FinishReason)
}

func TestStreamAdapter_CodeExecutionPairedByIDAcrossInterleavedParts(t *testing.T) {
	t.Parallel()

	parts := func(ps ...*genai.Part) *genai.GenerateContentResponse {
		return &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{Content: &genai.Content{Parts: ps}}}}
	}
	adapter := NewStreamAdapter(iterOf(
		parts(
			&genai.Part{ExecutableCode: &genai.ExecutableCode{ID: "a", Language: genai.LanguagePython, Code: "slow()"}},
			&genai.Part{ExecutableCode: &genai.ExecutableCode{ID: "b", Language: genai.LanguagePython, Code: "fast()"}},
		),
		parts(&genai.Part{CodeExecutionResult: &genai.CodeExecutionResult{ID: "b", Outcome: genai.OutcomeOK, Output: "fast"}}),
		parts(&genai.Part{CodeExecutionResult: &genai.CodeExecutionResult{ID: "a", Outcome: genai.OutcomeOK, Output: "slow"}}),
	), "m", false)

	deltas, final := drain(t, adapter)

	require.Len(t, deltas, 3)
	assert.Empty(t, deltas[0].ServerToolCalls)
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "fast()", Output: "fast"}}, deltas[1].ServerToolCalls)
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "slow()", Output: "slow"}}, deltas[2].ServerToolCalls)
	assert.Empty(t, final.Choices[0].Delta.ServerToolCalls, "nothing left to flush once every result was paired")
}

func TestStreamAdapter_IDLessCodeResultsPairWithinCandidate(t *testing.T) {
	t.Parallel()

	code := func(src string) *genai.Content {
		return &genai.Content{Parts: []*genai.Part{{ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: src}}}}
	}
	result := func(out string) *genai.Content {
		return &genai.Content{Parts: []*genai.Part{{CodeExecutionResult: &genai.CodeExecutionResult{Outcome: genai.OutcomeOK, Output: out}}}}
	}
	adapter := NewStreamAdapter(iterOf(
		&genai.GenerateContentResponse{Candidates: []*genai.Candidate{{Index: 0, Content: code("first()")}, {Index: 1, Content: code("second()")}}},
		// Only the second candidate streams a result on this chunk: its
		// range position is 0 but its Index is still 1.
		&genai.GenerateContentResponse{Candidates: []*genai.Candidate{{Index: 1, Content: result("2")}}},
		&genai.GenerateContentResponse{Candidates: []*genai.Candidate{{Index: 0, Content: result("1")}}},
	), "m", false)

	deltas, final := drain(t, adapter)

	require.Len(t, deltas, 3)
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "second()", Output: "2"}},
		deltas[1].ServerToolCalls, "ID-less results pair by candidate.Index, not by position in the chunk")
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "first()", Output: "1"}},
		deltas[2].ServerToolCalls)
	assert.Empty(t, final.Choices[0].Delta.ServerToolCalls)
}

func TestStreamAdapter_IDLessRepeatedCodeIsNotDeduplicated(t *testing.T) {
	t.Parallel()

	chunk := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{
			{ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: "roll()"}},
			{CodeExecutionResult: &genai.CodeExecutionResult{Outcome: genai.OutcomeOK, Output: "3"}},
			{ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: "roll()"}},
			{CodeExecutionResult: &genai.CodeExecutionResult{Outcome: genai.OutcomeOK, Output: "5"}},
		}},
	}}}

	deltas, _ := drain(t, NewStreamAdapter(iterOf(chunk), "m", false))

	require.Len(t, deltas, 1)
	assert.Equal(t, []chat.ServerToolCall{
		{Name: "code_execution", Language: "python", Input: "roll()", Output: "3"},
		{Name: "code_execution", Language: "python", Input: "roll()", Output: "5"},
	}, deltas[0].ServerToolCalls, "without IDs the same code can legitimately run twice")
}

func TestStreamAdapter_RepeatedIDBearingPartsReportedOnce(t *testing.T) {
	t.Parallel()

	parts := func(ps ...*genai.Part) *genai.GenerateContentResponse {
		return &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{Content: &genai.Content{Parts: ps}}}}
	}
	code := &genai.Part{ExecutableCode: &genai.ExecutableCode{ID: "c1", Language: genai.LanguagePython, Code: "print(1)"}}
	result := &genai.Part{CodeExecutionResult: &genai.CodeExecutionResult{ID: "c1", Outcome: genai.OutcomeOK, Output: "1\n"}}
	call := &genai.Part{ToolCall: &genai.ToolCall{ID: "t1", ToolType: genai.ToolTypeURLContext, Args: map[string]any{"urls": []any{"https://a.example"}}}}
	response := &genai.Part{ToolResponse: &genai.ToolResponse{ID: "t1", Response: map[string]any{"ok": true}}}

	// Gemini re-sends earlier parts on later chunks.
	adapter := NewStreamAdapter(iterOf(
		parts(code, call),
		parts(code, result, call),
		parts(code, result, call, response),
		parts(result, response),
	), "m", false)

	deltas, final := drain(t, adapter)

	require.Len(t, deltas, 4)
	assert.Empty(t, deltas[0].ServerToolCalls)
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "print(1)", Output: "1\n"}}, deltas[1].ServerToolCalls)
	assert.Equal(t, []chat.ServerToolCall{{Name: "url_context", Input: `{"urls":["https://a.example"]}`, Output: `{"ok":true}`}}, deltas[2].ServerToolCalls)
	assert.Empty(t, deltas[3].ServerToolCalls, "repeated results are dropped")
	assert.Empty(t, final.Choices[0].Delta.ServerToolCalls, "repeated invocations must not linger as unfinished")
}

func TestStreamAdapter_CodeAndToolIDsDoNotCollide(t *testing.T) {
	t.Parallel()

	chunk := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{
			{ToolCall: &genai.ToolCall{ID: "1", ToolType: genai.ToolTypeGoogleSearchWeb, Args: map[string]any{"q": "x"}}},
			{ExecutableCode: &genai.ExecutableCode{ID: "1", Language: genai.LanguagePython, Code: "compute()"}},
			{CodeExecutionResult: &genai.CodeExecutionResult{ID: "1", Outcome: genai.OutcomeOK, Output: "42"}},
			{ToolResponse: &genai.ToolResponse{ID: "1", Response: map[string]any{"hits": float64(2)}}},
		}},
	}}}

	deltas, final := drain(t, NewStreamAdapter(iterOf(chunk), "m", false))

	require.Len(t, deltas, 1)
	assert.Equal(t, []chat.ServerToolCall{
		{Name: "code_execution", Language: "python", Input: "compute()", Output: "42"},
		{Name: "google_search", Input: `{"q":"x"}`, Output: `{"hits":2}`},
	}, deltas[0].ServerToolCalls)
	assert.Empty(t, final.Choices[0].Delta.ServerToolCalls)
}

func TestStreamAdapter_NilCandidatesAndPartsDoNotPanic(t *testing.T) {
	t.Parallel()

	chunks := []*genai.GenerateContentResponse{
		{Candidates: []*genai.Candidate{nil}},
		{Candidates: []*genai.Candidate{nil, {Content: &genai.Content{Parts: []*genai.Part{nil}}}}},
		{Candidates: []*genai.Candidate{{Content: &genai.Content{Parts: []*genai.Part{
			nil,
			{ExecutableCode: &genai.ExecutableCode{Language: genai.LanguagePython, Code: "go()"}},
			nil,
			{CodeExecutionResult: &genai.CodeExecutionResult{Outcome: genai.OutcomeOK, Output: "ok"}},
			{Text: "done"},
			{FunctionCall: &genai.FunctionCall{Name: "lookup"}},
		}}}}},
	}
	for _, c := range chunks[:2] {
		assert.False(t, hasAnnotations(c))
	}

	deltas, final := drain(t, NewStreamAdapter(iterOf(chunks...), "m", false))

	require.Len(t, deltas, 1, "chunks with only nil candidates or parts carry nothing")
	assert.Equal(t, "done", deltas[0].Content)
	assert.Equal(t, []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "go()", Output: "ok"}}, deltas[0].ServerToolCalls)
	require.Len(t, deltas[0].ToolCalls, 1)
	assert.Equal(t, "lookup", deltas[0].ToolCalls[0].Function.Name)
	assert.Equal(t, chat.FinishReasonToolCalls, final.Choices[0].FinishReason)
}

func TestStreamAdapter_ServerSideToolCallsPairedByID(t *testing.T) {
	t.Parallel()

	parts := func(ps ...*genai.Part) *genai.GenerateContentResponse {
		return &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{Content: &genai.Content{Parts: ps}}}}
	}
	search := &genai.ToolCall{ID: "s1", ToolType: genai.ToolTypeGoogleSearchWeb, Args: map[string]any{"queries": []any{"weather paris"}}}
	fetch := &genai.ToolCall{ID: "u1", ToolType: genai.ToolTypeURLContext, Args: map[string]any{"urls": []any{"https://a.example"}}}
	grounded := groundedChunk("Sunny.", []string{"weather paris"}, webChunk("https://a.example", "A"))
	grounded.Candidates[0].URLContextMetadata = &genai.URLContextMetadata{URLMetadata: []*genai.URLMetadata{
		{RetrievedURL: "https://a.example", URLRetrievalStatus: genai.URLRetrievalStatusSuccess},
	}}

	adapter := NewStreamAdapter(iterOf(
		parts(&genai.Part{ToolCall: search}, &genai.Part{ToolCall: fetch}),
		parts(&genai.Part{ToolResponse: &genai.ToolResponse{ID: "u1", Response: map[string]any{"status": "ok"}}}),
		grounded,
		parts(&genai.Part{ToolResponse: &genai.ToolResponse{ID: "s1", ToolType: genai.ToolTypeGoogleSearchWeb, Response: map[string]any{"results": float64(3)}}}),
		parts(&genai.Part{ToolResponse: &genai.ToolResponse{ID: "orphan", ToolType: genai.ToolTypeGoogleMaps, Response: map[string]any{"place": "x"}}}),
	), "m", false)

	deltas, final := drain(t, adapter)

	require.Len(t, deltas, 5)
	assert.Empty(t, deltas[0].ServerToolCalls, "invocations wait for their responses")
	assert.Empty(t, deltas[0].ToolCalls, "server-side invocations never become local tool calls")
	assert.Equal(t, []chat.ServerToolCall{{Name: "url_context", Input: `{"urls":["https://a.example"]}`, Output: `{"status":"ok"}`}}, deltas[1].ServerToolCalls)
	assert.Empty(t, deltas[2].ServerToolCalls, "query and URL summaries are redundant once the invocations themselves are visible")
	assert.Equal(t, []chat.Citation{{URI: "https://a.example", Title: "A"}}, deltas[2].Citations)
	assert.Equal(t, "Sunny.", deltas[2].Content)
	assert.Equal(t, []chat.ServerToolCall{{Name: "google_search", Input: `{"queries":["weather paris"]}`, Output: `{"results":3}`}}, deltas[3].ServerToolCalls)
	assert.Equal(t, []chat.ServerToolCall{{Name: "google_maps", Output: `{"place":"x"}`}}, deltas[4].ServerToolCalls, "an unmatched response still shows up")
	assert.Empty(t, final.Choices[0].Delta.ServerToolCalls)
}

func TestStreamAdapter_URLContextMetadataStatuses(t *testing.T) {
	t.Parallel()

	withURLs := func(text string, urls ...*genai.URLMetadata) *genai.GenerateContentResponse {
		resp := textChunk(text)
		resp.Candidates[0].URLContextMetadata = &genai.URLContextMetadata{URLMetadata: urls}
		return resp
	}
	ok := &genai.URLMetadata{RetrievedURL: "https://ok.example", URLRetrievalStatus: genai.URLRetrievalStatusSuccess}
	paywall := &genai.URLMetadata{RetrievedURL: "https://paywall.example", URLRetrievalStatus: genai.URLRetrievalStatusPaywall}
	resp := withURLs("", ok, nil, &genai.URLMetadata{}, paywall)
	assert.True(t, hasAnnotations(resp))

	deltas, _ := drain(t, NewStreamAdapter(iterOf(resp, withURLs("Read it.", ok, paywall)), "m", false))

	require.Len(t, deltas, 2)
	assert.Equal(t, []chat.ServerToolCall{
		{Name: "url_context", Input: "https://ok.example", Output: "success"},
		{Name: "url_context", Input: "https://paywall.example", Output: "paywall", IsError: true},
	}, deltas[0].ServerToolCalls)
	assert.Empty(t, deltas[1].ServerToolCalls, "repeated metadata is reported once")
	assert.Equal(t, "Read it.", deltas[1].Content)
}

func TestServerToolName(t *testing.T) {
	t.Parallel()

	assert.Equal(t, "google_search", serverToolName(genai.ToolTypeGoogleSearchWeb))
	assert.Equal(t, "google_search_image", serverToolName(genai.ToolTypeGoogleSearchImage))
	assert.Equal(t, "url_context", serverToolName(genai.ToolTypeURLContext))
	assert.Equal(t, "google_maps", serverToolName(genai.ToolTypeGoogleMaps))
	assert.Equal(t, "server_tool", serverToolName(""))
	assert.Equal(t, "server_tool", serverToolName(genai.ToolTypeUnspecified))
}

func TestStreamAdapter_GroundingMetadataDeduplicatedAcrossChunks(t *testing.T) {
	t.Parallel()

	a := webChunk("https://a.example", "A")
	b := webChunk("https://b.example", "B")
	adapter := NewStreamAdapter(iterOf(
		groundedChunk("Paris ", []string{"weather paris"}, a),
		groundedChunk("is sunny", []string{"weather paris"}, a, b),
		groundedChunk(" today.", []string{"weather paris", "paris forecast"}, a, b),
	), "m", false)

	deltas, _ := drain(t, adapter)

	require.Len(t, deltas, 3)
	assert.Equal(t, []chat.Citation{{URI: "https://a.example", Title: "A"}}, deltas[0].Citations)
	assert.Equal(t, []chat.ServerToolCall{{Name: "google_search", Input: "weather paris"}}, deltas[0].ServerToolCalls)
	assert.Equal(t, []chat.Citation{{URI: "https://b.example", Title: "B"}}, deltas[1].Citations)
	assert.Empty(t, deltas[1].ServerToolCalls)
	assert.Empty(t, deltas[2].Citations)
	assert.Equal(t, []chat.ServerToolCall{{Name: "google_search", Input: "paris forecast"}}, deltas[2].ServerToolCalls)
	assert.Equal(t, "Paris ", deltas[0].Content, "text content is untouched by grounding metadata")
}

func TestStreamAdapter_CitationMetadataAndOtherChunkKinds(t *testing.T) {
	t.Parallel()

	resp := textChunk("quoted")
	resp.Candidates[0].CitationMetadata = &genai.CitationMetadata{Citations: []*genai.Citation{
		{URI: "https://quoted.example", Title: "Quoted"},
		nil,
		{Title: "no uri"},
	}}
	resp.Candidates[0].GroundingMetadata = &genai.GroundingMetadata{GroundingChunks: []*genai.GroundingChunk{
		nil,
		{},
		{RetrievedContext: &genai.GroundingChunkRetrievedContext{URI: "gs://bucket/doc", Title: "Doc"}},
		{Maps: &genai.GroundingChunkMaps{URI: "https://maps.example/place", Title: "Place"}},
		{Web: &genai.GroundingChunkWeb{URI: "https://quoted.example", Title: "dup of citation"}},
	}}

	deltas, _ := drain(t, NewStreamAdapter(iterOf(resp), "m", false))

	require.Len(t, deltas, 1)
	assert.Equal(t, []chat.Citation{
		{URI: "gs://bucket/doc", Title: "Doc"},
		{URI: "https://maps.example/place", Title: "Place"},
		{URI: "https://quoted.example", Title: "dup of citation"},
	}, deltas[0].Citations)
}

func TestStreamAdapter_MetadataOnlyChunkWithNils(t *testing.T) {
	t.Parallel()

	resp := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{
		{Content: nil, GroundingMetadata: &genai.GroundingMetadata{GroundingChunks: []*genai.GroundingChunk{webChunk("https://x.example", "")}}},
		{Content: &genai.Content{}},
	}}
	assert.True(t, hasAnnotations(resp))
	assert.False(t, hasAnnotations(textChunk("plain")))
	assert.False(t, hasAnnotations(&genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		GroundingMetadata: &genai.GroundingMetadata{}, CitationMetadata: &genai.CitationMetadata{},
	}}}), "empty metadata containers are not annotations")

	deltas, final := drain(t, NewStreamAdapter(iterOf(resp), "m", false))

	require.Len(t, deltas, 1)
	assert.Equal(t, []chat.Citation{{URI: "https://x.example"}}, deltas[0].Citations)
	assert.Empty(t, deltas[0].Content)
	assert.Equal(t, chat.FinishReasonStop, final.Choices[0].FinishReason)
}

func TestStreamAdapter_FunctionCallsUnaffectedByGrounding(t *testing.T) {
	t.Parallel()

	resp := &genai.GenerateContentResponse{Candidates: []*genai.Candidate{{
		Content: &genai.Content{Parts: []*genai.Part{{
			FunctionCall:     &genai.FunctionCall{Name: "lookup", Args: map[string]any{"q": "x"}},
			ThoughtSignature: []byte("sig"),
		}}},
		GroundingMetadata: &genai.GroundingMetadata{GroundingChunks: []*genai.GroundingChunk{webChunk("https://g.example", "G")}},
	}}}

	deltas, final := drain(t, NewStreamAdapter(iterOf(resp), "m", false))

	require.Len(t, deltas, 1)
	require.Len(t, deltas[0].ToolCalls, 1)
	assert.Equal(t, "lookup", deltas[0].ToolCalls[0].Function.Name)
	assert.Equal(t, []byte("sig"), deltas[0].ThoughtSignature)
	assert.Equal(t, []chat.Citation{{URI: "https://g.example", Title: "G"}}, deltas[0].Citations)
	assert.Equal(t, chat.FinishReasonToolCalls, final.Choices[0].FinishReason)
}

// TestConvertMessagesToGemini_AnnotationsNeverReplayed: persisted citations
// and server tool calls are display-only and must not alter what is sent
// back to the model.
func TestConvertMessagesToGemini_AnnotationsNeverReplayed(t *testing.T) {
	t.Parallel()

	plain := []chat.Message{
		{Role: chat.MessageRoleUser, Content: "2+2?"},
		{Role: chat.MessageRoleAssistant, Content: "4"},
	}
	annotated := []chat.Message{
		plain[0],
		{
			Role:            chat.MessageRoleAssistant,
			Content:         "4",
			Citations:       []chat.Citation{{URI: "https://math.example", Title: "Sums"}},
			ServerToolCalls: []chat.ServerToolCall{{Name: "code_execution", Language: "python", Input: "print(2+2)", Output: "4\n"}},
		},
	}
	store := modelsdev.NewDatabaseStore(&modelsdev.Database{})

	assert.Equal(t,
		convertMessagesToGemini(t.Context(), plain, modelsdev.ID{}, store, nil),
		convertMessagesToGemini(t.Context(), annotated, modelsdev.ID{}, store, nil),
	)
}

// TestCreateChatCompletionStream_GroundingAndCodeExecution parses the real
// wire shape Gemini uses for search grounding and code execution.
func TestCreateChatCompletionStream_GroundingAndCodeExecution(t *testing.T) {
	t.Parallel()

	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		for _, event := range []string{
			`data: {"candidates":[{"content":{"parts":[{"executableCode":{"id":"c1","language":"PYTHON","code":"print(2+2)"}}],"role":"model"}}]}` + "\n\n",
			`data: {"candidates":[{"content":{"parts":[{"codeExecutionResult":{"id":"c1","outcome":"OUTCOME_OK","output":"4\n"}}],"role":"model"}}]}` + "\n\n",
			`data: {"candidates":[{"content":{"parts":[{"toolCall":{"id":"t1","toolType":"URL_CONTEXT","args":{"urls":["https://math.example/sum"]}}}],"role":"model"}}]}` + "\n\n",
			`data: {"candidates":[{"content":{"parts":[{"toolResponse":{"id":"t1","toolType":"URL_CONTEXT","response":{"fetched":1}}}],"role":"model"}}]}` + "\n\n",
			`data: {"candidates":[{"content":{"parts":[{"text":"It is 4."}],"role":"model"},"groundingMetadata":{"webSearchQueries":["2+2"],"groundingChunks":[{"web":{"uri":"https://math.example/sum","title":"Sums"}}]},"urlContextMetadata":{"urlMetadata":[{"retrievedUrl":"https://math.example/sum","urlRetrievalStatus":"URL_RETRIEVAL_STATUS_SUCCESS"}]}}]}` + "\n\n",
			`data: {"candidates":[{"content":{"parts":[{"text":" Done."}],"role":"model"},"finishReason":"STOP","groundingMetadata":{"webSearchQueries":["2+2"],"groundingChunks":[{"web":{"uri":"https://math.example/sum","title":"Sums"}}]}}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":3,"totalTokenCount":5}}` + "\n\n",
		} {
			_, _ = io.WriteString(w, event)
			w.(http.Flusher).Flush()
		}
	}))
	t.Cleanup(server.Close)

	cfg := &latest.ModelConfig{Provider: "google", Model: "gemini-3.8-flash", BaseURL: server.URL}
	env := environment.NewMapEnvProvider(map[string]string{"GOOGLE_API_KEY": "test-key"})
	client, err := NewClient(t.Context(), cfg, env)
	require.NoError(t, err)

	stream, err := client.CreateChatCompletionStream(t.Context(), []chat.Message{{Role: chat.MessageRoleUser, Content: "2+2?"}}, nil)
	require.NoError(t, err)
	t.Cleanup(stream.Close)

	var content strings.Builder
	var citations []chat.Citation
	var calls []chat.ServerToolCall
	var finish chat.FinishReason
	for finish == "" {
		resp, err := stream.Recv()
		require.NoError(t, err)
		require.Len(t, resp.Choices, 1)
		delta := resp.Choices[0].Delta
		content.WriteString(delta.Content)
		citations, _ = chat.MergeCitations(citations, delta.Citations)
		calls = append(calls, delta.ServerToolCalls...)
		finish = resp.Choices[0].FinishReason
	}

	assert.Equal(t, "It is 4. Done.", content.String())
	assert.Equal(t, []chat.Citation{{URI: "https://math.example/sum", Title: "Sums"}}, citations)
	assert.Equal(t, []chat.ServerToolCall{
		{Name: "code_execution", Language: "python", Input: "print(2+2)", Output: "4\n"},
		{Name: "url_context", Input: `{"urls":["https://math.example/sum"]}`, Output: `{"fetched":1}`},
		{Name: "google_search", Input: "2+2"},
	}, calls, "the explicit url_context invocation supersedes its metadata summary")
	assert.Equal(t, chat.FinishReasonStop, finish)
}
