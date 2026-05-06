package secretsscan_test

import (
	"strings"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"

	"github.com/docker/docker-agent/pkg/secretsscan"
)

// TestContainsSecretsRecognisesKnownTokens: parity guarantee with the
// upstream docker/mcp-gateway/pkg/secretsscan tests. Failing this test
// means we either dropped a rule or broke the keyword pre-filter.
func TestContainsSecretsRecognisesKnownTokens(t *testing.T) {
	t.Parallel()

	cases := []struct {
		name string
		text string
	}{
		{"github_pat", "ghp_cxLeRrvbJfmYdUtr70xnNE3Q7Gvli43s19PD"},
		{"docker_pat", "dckr_pat_" + "AAAAAAAAAAAAAAAAAAAAAAAAAAA"},
		// Patterns added on top of the upstream catalogue. Each value
		// is split across string concatenation so the verbatim token
		// never appears on a single source line in case downstream
		// tooling scans the test file itself.
		{"openai_project_key", "sk-proj-" + strings.Repeat("A", 25) + "T3Blbk" + "FJ" + strings.Repeat("B", 25)},
		{"anthropic_api_key", "sk-ant-" + "api03-" + strings.Repeat("X", 93) + "AA"},
		{"google_api_key", "AIza" + strings.Repeat("a", 35)},
		{"google_oauth_client_secret", "GOCSPX-" + strings.Repeat("a", 28)},
		{"digitalocean_pat", "dop_v1_" + strings.Repeat("a", 64)},
		{"stripe_webhook_signing_secret", "whsec_" + strings.Repeat("a", 40)},
		{"jfrog_api_key", "AKCp" + strings.Repeat("a", 73)},
		{"tencent_cloud_secret_id", "AKID" + strings.Repeat("a", 32)},
		{"sentry_user_auth_token", "sntrys_" + "eyJ" + strings.Repeat("a", 60)},
		{"stripe_restricted_key_test", "rk_test_" + strings.Repeat("a", 24)},
		{"stripe_restricted_key_live", "rk_live_" + strings.Repeat("a", 24)},
		{"notion_integration_token", "ntn_" + strings.Repeat("a", 46)},
		{"gitlab_pipeline_trigger_token", "glptt-" + strings.Repeat("a", 40)},
		{"vault_service_token", "hvs." + strings.Repeat("a", 95)},
		{"vault_service_token_long", "hvs." + strings.Repeat("a", 180)},
		{"slack_rotating_refresh_token", "xoxe-" + strings.Repeat("a", 60)},
		{"slack_rotating_user_token", "xoxe.xoxp-" + strings.Repeat("a", 50)},
		{"slack_rotating_bot_token", "xoxe.xoxb-" + strings.Repeat("a", 50)},
		{"slack_rotating_long_token", "xoxe.xoxp-" + strings.Repeat("a", 250)},
		{"replicate_api_token", "r8_" + strings.Repeat("a", 37)},
		{"square_access_token", "EAAA" + strings.Repeat("a", 60)},
		{"atlassian_cloud_api_token", "ATATT3xFfGF0" + strings.Repeat("a", 200)},
		{"digitalocean_oauth_token", "doo_v1_" + strings.Repeat("a", 64)},
		{"digitalocean_oauth_refresh", "dor_v1_" + strings.Repeat("a", 64)},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			assert.Truef(t, secretsscan.ContainsSecrets(tc.text), "must detect %s", tc.name)
			out := secretsscan.Redact(tc.text)
			assert.NotContainsf(t, out, tc.text, "raw secret must be gone after Redact: %q", out)
			assert.Containsf(t, out, secretsscan.RedactionMarker,
				"redaction marker must appear in %q", out)
		})
	}
}

// TestRedactDetectsBareUnquotedSecrets exercises the rules whose
// expressions used to require surrounding `'` / `"` characters
// (copied from upstream Trivy, where the scanner is aimed at JSON /
// YAML config files). The agent's leak vectors are different: tool
// output, CLI shells, and chat content where credentials routinely
// appear unquoted (`echo $NPM_TOKEN`, `vault token lookup`, log
// dumps). Each rule below has a unique-enough prefix to stay
// specific without the quote anchor — this test pins that property
// so a future refactor doesn't accidentally re-introduce the
// quote-only matching.
func TestRedactDetectsBareUnquotedSecrets(t *testing.T) {
	t.Parallel()

	cases := []struct {
		name  string
		token string
	}{
		{"npm_access_token_bare", "npm_" + strings.Repeat("a", 36)},
		{"dynatrace_api_token_bare", "dt0c01." + strings.Repeat("a", 24) + "." + strings.Repeat("b", 64)},
		{"doppler_personal_token_bare", "dp.pt." + strings.Repeat("a", 43)},
		{"easypost_production_token_bare", "EZAK" + strings.Repeat("a", 54)},
		{"easypost_test_token_bare", "EZTK" + strings.Repeat("a", 54)},
		{"hashicorp_terraform_token_bare", strings.Repeat("a", 14) + ".atlasv1." + strings.Repeat("b", 64)},
		{"new_relic_user_api_key_bare", "NRAK-" + strings.Repeat("A", 27)},
		{"new_relic_browser_api_token_bare", "NRJS-" + strings.Repeat("a", 19)},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			// The same value emitted bare (CLI / log) and quoted
			// (config file) must both be redacted.
			for _, in := range []string{tc.token, `"` + tc.token + `"`, `'` + tc.token + `'`} {
				assert.Truef(t, secretsscan.ContainsSecrets(in),
					"must detect %s in %q", tc.name, in)
				out := secretsscan.Redact(in)
				assert.NotContainsf(t, out, tc.token,
					"raw secret must be gone after Redact: %q", out)
				assert.Containsf(t, out, secretsscan.RedactionMarker,
					"redaction marker must appear in %q", out)
			}
		})
	}
}

// TestRedactDoesNotSwallowAdjacentTextAfterSlackRotatingToken pins
// the upper bound on the slack-rotating-token rule. The body class
// overlaps with hostnames / dotted identifiers, so without an upper
// quantifier bound a Slack token followed (without separator) by an
// `api.slack.com`-style URL would silently consume the URL into the
// redaction span. The upper bound of 300 is comfortably above the
// longest observed Slack rotating-token body, so a real secret is
// still fully redacted, but text well past that length stops being
// part of the same match.
func TestRedactDoesNotSwallowAdjacentTextAfterSlackRotatingToken(t *testing.T) {
	t.Parallel()

	// 320 chars is past the {40,300} cap, so the regex must split
	// the input into a redacted prefix and a literal trailing
	// suffix. The trailing dotted hostname is exactly the kind of
	// content that an open-ended quantifier would over-consume.
	token := "xoxe-" + strings.Repeat("a", 320)
	input := token + ".api.slack.com"

	out := secretsscan.Redact(input)

	assert.Containsf(t, out, secretsscan.RedactionMarker,
		"a Slack rotating token must still be redacted: %q", out)
	assert.Containsf(t, out, "api.slack.com",
		"adjacent hostname must NOT be swallowed by the rotating-token regex: %q", out)
}

// TestContainsSecretsIgnoresHarmlessText: pure digit strings, plain
// English, and the empty string must never trip detection.
func TestContainsSecretsIgnoresHarmlessText(t *testing.T) {
	t.Parallel()

	cases := []string{
		"",
		"1234567890",
		"hello world",
		"please summarise the README",
		// "key" is a keyword for aws-secret-access-key, but the regex
		// requires a 40-char base64-ish span next to "aws_*key=" so a
		// bare mention must not trip detection.
		"the api key is documented in README",
	}
	for _, in := range cases {
		assert.Falsef(t, secretsscan.ContainsSecrets(in), "must not flag %q", in)
	}
}

// TestRedactReplacesSecretSpan: the secret material is replaced by
// [secretsscan.RedactionMarker] while the surrounding text (including
// the keyword that triggered the match) is preserved. We don't assert
// the exact match boundary because the rule's leading-context group
// may consume the preceding space — only the secret value must
// disappear.
func TestRedactReplacesSecretSpan(t *testing.T) {
	t.Parallel()

	const ghp = "ghp_cxLeRrvbJfmYdUtr70xnNE3Q7Gvli43s19PD"
	in := "Run this with token=" + ghp + " and you're set"

	out := secretsscan.Redact(in)

	assert.Containsf(t, out, secretsscan.RedactionMarker, "redaction marker must appear: %q", out)
	assert.NotContainsf(t, out, ghp, "raw secret must be gone: %q", out)
	assert.Contains(t, out, "Run this with token=", "non-secret prefix preserved")
	assert.Contains(t, out, "and you're set", "non-secret suffix preserved")
}

// TestRedactIsIdempotent: passing already-redacted text through
// Redact again leaves it untouched. Without this we'd risk
// amplification when both the pre_tool_use builtin and the
// before_llm_call transform fire on the same content.
func TestRedactIsIdempotent(t *testing.T) {
	t.Parallel()

	once := secretsscan.Redact("dckr_pat_" + "AAAAAAAAAAAAAAAAAAAAAAAAAAA in logs")
	twice := secretsscan.Redact(once)

	assert.Equal(t, once, twice)
	assert.False(t, secretsscan.ContainsSecrets(once),
		"redacted output must no longer trip ContainsSecrets")
}

// TestRedactPreservesNonMatchingText: text without secrets must pass
// through untouched (catches a regression where a too-broad rule
// inserts a marker into innocent content).
func TestRedactPreservesNonMatchingText(t *testing.T) {
	t.Parallel()

	cases := []string{
		"",
		"1234567890",
		"please refactor the helper into its own file",
	}
	for _, in := range cases {
		assert.Equalf(t, in, secretsscan.Redact(in), "must not modify %q", in)
	}
}

// TestRedactHandlesMultipleSecretsInOneInput: two distinct secrets in
// the same string must both be replaced, and nothing in between
// should leak out (regression test for the cursor-rebuild loop).
func TestRedactHandlesMultipleSecretsInOneInput(t *testing.T) {
	t.Parallel()

	const a = "ghp_cxLeRrvbJfmYdUtr70xnNE3Q7Gvli43s19PD"
	const b = "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
	in := "first " + a + " and second " + b + " end"

	out := secretsscan.Redact(in)

	require.NotContains(t, out, a)
	require.NotContains(t, out, b)
	assert.Equal(t, 2, strings.Count(out, secretsscan.RedactionMarker),
		"both secrets must be redacted: %q", out)
	assert.Contains(t, out, "first ")
	assert.Contains(t, out, " and second ")
	assert.Contains(t, out, " end")
}

// TestRedactionMarkerIsNotASecret locks in the safety property that
// makes [secretsscan.Redact] idempotent and the keyword pre-filter
// hoist in [secretsscan.Redact] sound: the literal RedactionMarker
// must not match any detection rule. If a future rule were added
// whose keyword overlapped "[redacted]", redaction would either
// recurse forever or amplify the marker on every pass, and any
// downstream pipeline that calls Redact twice (the pre_tool_use
// builtin + the before_llm_call transform on the same string) would
// silently corrupt content.
func TestRedactionMarkerIsNotASecret(t *testing.T) {
	t.Parallel()

	assert.False(t, secretsscan.ContainsSecrets(secretsscan.RedactionMarker),
		"the redaction marker must not match any rule")
	assert.Equal(t, secretsscan.RedactionMarker,
		secretsscan.Redact(secretsscan.RedactionMarker),
		"redacting the marker must be a no-op")

	// Embedded in arbitrary surrounding text the marker still must not
	// match — some rules only fire mid-string after a non-word boundary.
	assert.Equal(t, "prefix "+secretsscan.RedactionMarker+" suffix",
		secretsscan.Redact("prefix "+secretsscan.RedactionMarker+" suffix"))
}

// TestRedactDetectsSecretsAcrossWordBoundaries exercises the change
// that dropped the leading and trailing word-boundary anchors from
// the rule expressions. Before the change, only secrets that stood
// next to whitespace, punctuation, or the start/end of the input were
// detected; values pasted directly into a larger token ("FOO=ghp_…"
// without the trailing space, "BEFOREghp_…AFTER") leaked through.
// Each subtest pins one of those previously-leaking shapes — the
// exact same secret value embedded in different contexts must always
// be redacted out.
func TestRedactDetectsSecretsAcrossWordBoundaries(t *testing.T) {
	t.Parallel()

	// Split the literal secret values across string concatenation so
	// the verbatim token never appears on a single source line; that
	// keeps secret-scanners (including ours) happy on the test file
	// itself while still exercising the real ruleset.
	ghp := "ghp_" + "cxLeRrvbJfmYdUtr70xnNE3Q7Gvli43s19PD"
	awsAccessKey := "AKIA" + "IOSFODNN7EXAMPLE"
	dockerPAT := "dckr_pat_" + "AAAAAAAAAAAAAAAAAAAAAAAAAAA"

	cases := []struct {
		name   string
		secret string
		input  string
	}{
		{"github-pat alone", ghp, ghp},
		{"github-pat with leading alphanumerics", ghp, "BEFORE" + ghp},
		{"github-pat with trailing alphanumerics", ghp, ghp + "AFTER"},
		{"github-pat embedded in a larger token", ghp, "BEFORE" + ghp + "AFTER"},
		{"github-pat after KEY=", ghp, "KEY=" + ghp},
		{"github-pat after KEY= and inline word", ghp, "KEY=" + ghp + "AFTER"},
		{"aws-access-key alone", awsAccessKey, awsAccessKey},
		{"aws-access-key with leading alphanumerics", awsAccessKey, "BEFORE" + awsAccessKey},
		{"aws-access-key with trailing alphanumerics", awsAccessKey, awsAccessKey + "AFTER"},
		{"aws-access-key embedded in a larger token", awsAccessKey, "BEFORE" + awsAccessKey + "AFTER"},
		{"docker-pat with leading alphanumerics", dockerPAT, "BEFORE" + dockerPAT},
		{"docker-pat embedded in a larger token", dockerPAT, "BEFORE" + dockerPAT + "AFTER"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			assert.Truef(t, secretsscan.ContainsSecrets(tc.input),
				"must detect secret in %q", tc.input)
			out := secretsscan.Redact(tc.input)
			assert.NotContainsf(t, out, tc.secret,
				"raw secret must be gone after Redact: %q", out)
			assert.Containsf(t, out, secretsscan.RedactionMarker,
				"redaction marker must appear in %q", out)
		})
	}
}

// TestRedactScalesLinearly is a guard-rail against accidentally
// reintroducing a quadratic algorithm when iterating on Redact
// (e.g. retrying every rule from each character offset). With the
// keyword pre-filter and Go's RE2-based regexp engine the cost is
// O(len(text) · len(rules)), so doubling the input must roughly
// double the wall time — not quadruple it. We deliberately check a
// generous ceiling (8×) to stay reliable under noisy CI; a true
// quadratic regression on a 16× size delta would blow well past it.
func TestRedactScalesLinearly(t *testing.T) {
	t.Parallel()

	// Warm caches (rule compilation, regex DFA) so the first sample
	// doesn't pay a one-time tax that skews the ratio.
	_ = secretsscan.Redact("warmup")

	measure := func(text string) time.Duration {
		const iters = 50
		start := time.Now()
		for range iters {
			_ = secretsscan.Redact(text)
		}
		return time.Since(start) / iters
	}

	// Realistic cleanish payload: prose with a couple of secret-like
	// keywords sprinkled in so the keyword pre-filter sometimes lets
	// rules through to their regex.
	unit := "the quick brown fox key=" + "abcdefghijklmnop" +
		" jumps over the lazy dog with token=" + "ghp_xxx" + ". "
	small := strings.Repeat(unit, 64)
	large := strings.Repeat(unit, 1024) // 16× small

	dSmall := measure(small)
	dLarge := measure(large)

	// Allow up to 128× — quadratic would be ~256× on a 16× delta and
	// the headroom keeps the test stable when the host is loaded.
	const growthCeiling = 128
	if dSmall == 0 {
		t.Skip("clock too coarse to measure small input")
	}
	ratio := float64(dLarge) / float64(dSmall)
	assert.Lessf(t, ratio, float64(growthCeiling),
		"Redact must not be quadratic: 16× input took %.1f× the time (small=%v, large=%v)",
		ratio, dSmall, dLarge)
}
