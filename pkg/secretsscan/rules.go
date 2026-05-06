// The rules in this file are derived from the MIT-licensed
// github.com/docker/mcp-gateway/pkg/secretsscan package, which itself
// copied the patterns from
// github.com/aquasecurity/trivy/pkg/fanal/secret/builtin-rules.go.
//
// Copyright (c) 2025 Docker (MIT). Re-licensed here under Apache-2.0
// per the license-compatibility allowances of the MIT License.

package secretsscan

import (
	"fmt"
	"regexp"
	"strings"
	"sync"
)

const (
	quote   = `["']?`
	connect = `\s*(:|=>|=)?\s*`
	aws     = `aws_?`
)

// rule pairs a regular expression with a keyword shortlist. A rule
// matches when the (lower-cased) input contains any of the keywords
// AND the (case-sensitive) expression matches; the keyword filter is
// what keeps detection fast for typical inputs.
type rule struct {
	expression string
	keywords   []string
}

// asSecretGroup wraps a `?P<secret>…` fragment in a plain group so
// the named subgroup is syntactically valid. Earlier revisions also
// prepended a `[^0-9a-zA-Z]|^` anchor and appended a
// whitespace/punctuation/end-of-input anchor (collectively a
// "word boundary" requirement) so a rule only fired when the secret
// stood alone in the input. Those anchors caused detection to miss
// secrets embedded directly inside larger tokens — e.g.
// `BEFOREghp_…AFTER`, `KEY=AKIA…`, `…EXAMPLEAFTER` — even though the
// recognisable prefix and exact-length payload were both present.
//
// Detection now ignores the surrounding characters entirely. Each
// rule's payload is tightly constrained (fixed-length character
// classes, explicit token shapes) so removing the boundary check
// does not broaden the regex enough to cause super-linear matching:
// Go's RE2-based engine still scans the input in O(len(text)) per
// rule, and the keyword pre-filter in [Redact] keeps the regex hot
// path off most inputs.
func asSecretGroup(str string) string {
	return fmt.Sprintf("(%s)", str)
}

// rules is the source-form catalogue, kept verbatim from upstream so
// future updates apply cleanly. [compiledRuleSet] resolves it into
// the regex-compiled form actually used at scan time.
//
//nolint:funlen // single-source-of-truth for the ruleset
var rules = sync.OnceValue(func() []rule {
	return []rule{
		{
			// aws-access-key-id
			expression: asSecretGroup(`(?P<secret>(A3T[A-Z0-9]|AKIA|AGPA|AidA|AROA|AIPA|ANPA|ANVA|ASIA)[A-Z0-9]{16})` + quote),
			keywords:   []string{"AKIA", "AGPA", "AidA", "AROA", "AIPA", "ANPA", "ANVA", "ASIA"},
		},
		{
			// aws-secret-access-key
			expression: fmt.Sprintf(`(?i)%s%s(sec(ret)?)?_?(access)?_?key%s%s%s(?P<secret>[A-Za-z0-9\/\+=]{40})%s`, quote, aws, quote, connect, quote, quote),
			keywords:   []string{"key"},
		},
		{
			// github-pat
			expression: asSecretGroup(`?P<secret>ghp_[0-9a-zA-Z]{36}`),
			keywords:   []string{"ghp_"},
		},
		{
			// github-oauth
			expression: asSecretGroup(`?P<secret>gho_[0-9a-zA-Z]{36}`),
			keywords:   []string{"gho_"},
		},
		{
			// github-app-token
			expression: asSecretGroup(`?P<secret>(ghu|ghs)_[0-9a-zA-Z]{36}`),
			keywords:   []string{"ghu_", "ghs_"},
		},
		{
			// github-refresh-token
			expression: asSecretGroup(`?P<secret>ghr_[0-9a-zA-Z]{76}`),
			keywords:   []string{"ghr_"},
		},
		{
			// github-fine-grained-pat
			expression: `github_pat_[a-zA-Z0-9]{22}_[a-zA-Z0-9]{59}`,
			keywords:   []string{"github_pat_"},
		},
		{
			// gitlab-pat
			expression: asSecretGroup(`?P<secret>glpat-[0-9a-zA-Z\-\_]{20}`),
			keywords:   []string{"glpat-"},
		},
		{
			// hugging-face-access-token
			expression: asSecretGroup(`?P<secret>hf_[A-Za-z0-9]{34,40}`),
			keywords:   []string{"hf_"},
		},
		{
			// private-key
			expression: `(?i)-----\s*?BEGIN[ A-Z0-9_-]*?PRIVATE KEY( BLOCK)?\s*?-----[\s]*?(?P<secret>[A-Za-z0-9=+/\\\r\n][A-Za-z0-9=+/\\\s]+)[\s]*?-----\s*?END[ A-Z0-9_-]*? PRIVATE KEY( BLOCK)?\s*?-----`,
			keywords:   []string{"-----"},
		},
		{
			// shopify-token
			expression: `shp(ss|at|ca|pa)_[a-fA-F0-9]{32}`,
			keywords:   []string{"shpss_", "shpat_", "shpca_", "shppa_"},
		},
		{
			// slack-access-token
			expression: asSecretGroup(`?P<secret>xox[baprs]-([0-9a-zA-Z]{10,48})`),
			keywords:   []string{"xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"},
		},
		{
			// stripe-publishable-token
			expression: asSecretGroup(`?P<secret>(?i)pk_(test|live)_[0-9a-z]{10,32}`),
			keywords:   []string{"pk_test_", "pk_live_"},
		},
		{
			// stripe-secret-token
			expression: asSecretGroup(`?P<secret>(?i)sk_(test|live)_[0-9a-z]{10,32}`),
			keywords:   []string{"sk_test_", "sk_live_"},
		},
		{
			// pypi-upload-token
			expression: `pypi-AgEIcHlwaS5vcmc[A-Za-z0-9\-_]{50,1000}`,
			keywords:   []string{"pypi-AgEIcHlwaS5vcmc"},
		},
		{
			// gcp-service-account
			expression: `\"type\": \"service_account\"`,
			keywords:   []string{"\"type\": \"service_account\""},
		},
		{
			// heroku-api-key
			expression: ` (?i)(?P<key>heroku[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12})['\"]`,
			keywords:   []string{"heroku"},
		},
		{
			// slack-web-hook
			expression: `https:\/\/hooks.slack.com\/services\/[A-Za-z0-9+\/]{44,48}`,
			keywords:   []string{"hooks.slack.com"},
		},
		{
			// twilio-api-key
			expression: `SK[0-9a-fA-F]{32}`,
			keywords:   []string{"SK"},
		},
		{
			// age-secret-key
			expression: `AGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58}`,
			keywords:   []string{"AGE-SECRET-KEY-1"},
		},
		{
			// facebook-token
			expression: `(?i)(?P<key>facebook[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-f0-9]{32})['\"]`,
			keywords:   []string{"facebook"},
		},
		{
			// twitter-token
			expression: `(?i)(?P<key>twitter[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-f0-9]{35,44})['\"]`,
			keywords:   []string{"twitter"},
		},
		{
			// adobe-client-id
			expression: `(?i)(?P<key>adobe[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-f0-9]{32})['\"]`,
			keywords:   []string{"adobe"},
		},
		{
			// adobe-client-secret
			expression: `(p8e-)(?i)[a-z0-9]{32}`,
			keywords:   []string{"p8e-"},
		},
		{
			// alibaba-access-key-id
			expression: `(?P<secret>(LTAI)(?i)[a-z0-9]{20})`,
			keywords:   []string{"LTAI"},
		},
		{
			// alibaba-secret-key
			expression: `(?i)(?P<key>alibaba[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{30})['\"]`,
			keywords:   []string{"alibaba"},
		},
		{
			// asana-client-id
			expression: `(?i)(?P<key>asana[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[0-9]{16})['\"]`,
			keywords:   []string{"asana"},
		},
		{
			// asana-client-secret
			expression: `(?i)(?P<key>asana[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{32})['\"]`,
			keywords:   []string{"asana"},
		},
		{
			// atlassian-api-token
			expression: `(?i)(?P<key>atlassian[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{24})['\"]`,
			keywords:   []string{"atlassian"},
		},
		{
			// bitbucket-client-id
			expression: `(?i)(?P<key>bitbucket[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{32})['\"]`,
			keywords:   []string{"bitbucket"},
		},
		{
			// bitbucket-client-secret
			expression: `(?i)(?P<key>bitbucket[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9_\-]{64})['\"]`,
			keywords:   []string{"bitbucket"},
		},
		{
			// beamer-api-token
			expression: `(?i)(?P<key>beamer[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>b_[a-z0-9=_\-]{44})['\"]`,
			keywords:   []string{"beamer"},
		},
		{
			// clojars-api-token
			expression: `(CLOJARS_)(?i)[a-z0-9]{60}`,
			keywords:   []string{"CLOJARS_"},
		},
		{
			// contentful-delivery-api-token
			expression: `(?i)(?P<key>contentful[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9\-=_]{43})['\"]`,
			keywords:   []string{"contentful"},
		},
		{
			// databricks-api-token
			expression: `dapi[a-h0-9]{32}`,
			keywords:   []string{"dapi"},
		},
		{
			// discord-api-token
			expression: `(?i)(?P<key>discord[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-h0-9]{64})['\"]`,
			keywords:   []string{"discord"},
		},
		{
			// discord-client-id
			expression: `(?i)(?P<key>discord[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[0-9]{18})['\"]`,
			keywords:   []string{"discord"},
		},
		{
			// discord-client-secret
			expression: `(?i)(?P<key>discord[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9=_\-]{32})['\"]`,
			keywords:   []string{"discord"},
		},
		{
			// doppler-api-token. Personal tokens are `dp.pt.<43 chars>`;
			// the prefix is unique to Doppler so quotes aren't required.
			expression: `(dp\.pt\.)(?i)[a-z0-9]{43}`,
			keywords:   []string{"dp.pt."},
		},
		{
			// dropbox-api-secret
			expression: `(?i)(dropbox[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"]([a-z0-9]{15})['\"]`,
			keywords:   []string{"dropbox"},
		},
		{
			// dropbox-short-lived-api-token
			expression: `(?i)(dropbox[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](sl\.[a-z0-9\-=_]{135})['\"]`,
			keywords:   []string{"dropbox"},
		},
		{
			// dropbox-long-lived-api-token
			expression: `(?i)(dropbox[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"][a-z0-9]{11}(AAAAAAAAAA)[a-z0-9\-_=]{43}['\"]`,
			keywords:   []string{"dropbox"},
		},
		{
			// duffel-api-token
			expression: `['\"]duffel_(test|live)_(?i)[a-z0-9_-]{43}['\"]`,
			keywords:   []string{"duffel_test_", "duffel_live_"},
		},
		{
			// dynatrace-api-token. `dt0c01.` is the documented version
			// prefix; combined with the fixed 24+64 hex body it doesn't
			// need quote anchoring to stay specific.
			expression: `dt0c01\.(?i)[a-z0-9]{24}\.[a-z0-9]{64}`,
			keywords:   []string{"dt0c01."},
		},
		{
			// easypost-api-token. `EZAK` (production) / `EZTK` (test)
			// prefixes plus the fixed 54-char body are specific enough
			// that surrounding quotes aren't required.
			expression: `EZ[AT]K(?i)[a-z0-9]{54}`,
			keywords:   []string{"EZAK", "EZTK"},
		},
		{
			// fastly-api-token
			expression: `(?i)(?P<key>fastly[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9\-=_]{32})['\"]`,
			keywords:   []string{"fastly"},
		},
		{
			// finicity-client-secret
			expression: `(?i)(?P<key>finicity[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{20})['\"]`,
			keywords:   []string{"finicity"},
		},
		{
			// finicity-api-token
			expression: `(?i)(?P<key>finicity[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-f0-9]{32})['\"]`,
			keywords:   []string{"finicity"},
		},
		{
			// flutterwave-public-key
			expression: asSecretGroup(`?P<secret>FLW(PUB|SEC)K_TEST-(?i)[a-h0-9]{32}-X`),
			keywords:   []string{"FLWSECK_TEST-", "FLWPUBK_TEST-"},
		},
		{
			// flutterwave-enc-key
			expression: asSecretGroup(`?P<secret>FLWSECK_TEST[a-h0-9]{12}`),
			keywords:   []string{"FLWSECK_TEST"},
		},
		{
			// frameio-api-token
			expression: `fio-u-(?i)[a-z0-9\-_=]{64}`,
			keywords:   []string{"fio-u-"},
		},
		{
			// gocardless-api-token
			expression: `['\"]live_(?i)[a-z0-9\-_=]{40}['\"]`,
			keywords:   []string{"live_"},
		},
		{
			// grafana-api-token
			expression: `['\"]?eyJrIjoi(?i)[a-z0-9\-_=]{72,92}['\"]?`,
			keywords:   []string{"eyJrIjoi"},
		},
		{
			// hashicorp-tf-api-token. The `<14 chars>.atlasv1.<body>`
			// shape is documented and unique to Terraform Cloud, so the
			// quote anchors that the upstream rule used aren't needed.
			expression: `(?i)[a-z0-9]{14}\.atlasv1\.[a-z0-9\-_=]{60,70}`,
			keywords:   []string{"atlasv1."},
		},
		{
			// hubspot-api-token
			expression: `(?i)(?P<key>hubspot[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-h0-9]{8}-[a-h0-9]{4}-[a-h0-9]{4}-[a-h0-9]{4}-[a-h0-9]{12})['\"]`,
			keywords:   []string{"hubspot"},
		},
		{
			// intercom-api-token
			expression: `(?i)(?P<key>intercom[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9=_]{60})['\"]`,
			keywords:   []string{"intercom"},
		},
		{
			// intercom-client-secret
			expression: `(?i)(?P<key>intercom[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-h0-9]{8}-[a-h0-9]{4}-[a-h0-9]{4}-[a-h0-9]{4}-[a-h0-9]{12})['\"]`,
			keywords:   []string{"intercom"},
		},
		{
			// ionic-api-token
			expression: `(?i)(ionic[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](ion_[a-z0-9]{42})['\"]`,
			keywords:   []string{"ionic"},
		},
		{
			// jwt-token
			expression: `ey[a-zA-Z0-9]{17,}\.ey[a-zA-Z0-9\/\\_-]{17,}\.(?:[a-zA-Z0-9\/\\_-]{10,}={0,2})?`,
			keywords:   []string{".eyJ"},
		},
		{
			// linear-api-token
			expression: `lin_api_(?i)[a-z0-9]{40}`,
			keywords:   []string{"lin_api_"},
		},
		{
			// linear-client-secret
			expression: `(?i)(?P<key>linear[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-f0-9]{32})['\"]`,
			keywords:   []string{"linear"},
		},
		{
			// lob-api-key
			expression: `(?i)(?P<key>lob[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>(live|test)_[a-f0-9]{35})['\"]`,
			keywords:   []string{"lob"},
		},
		{
			// lob-pub-api-key
			expression: `(?i)(?P<key>lob[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>(test|live)_pub_[a-f0-9]{31})['\"]`,
			keywords:   []string{"lob"},
		},
		{
			// mailchimp-api-key
			expression: `(?i)(?P<key>mailchimp[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-f0-9]{32}-us20)['\"]`,
			keywords:   []string{"mailchimp"},
		},
		{
			// mailgun-token
			expression: `(?i)(?P<key>mailgun[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>(pub)?key-[a-f0-9]{32})['\"]`,
			keywords:   []string{"mailgun"},
		},
		{
			// mailgun-signing-key
			expression: `(?i)(?P<key>mailgun[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-h0-9]{32}-[a-h0-9]{8}-[a-h0-9]{8})['\"]`,
			keywords:   []string{"mailgun"},
		},
		{
			// mapbox-api-token
			expression: `(?i)(pk\.[a-z0-9]{60}\.[a-z0-9]{22})`,
			keywords:   []string{"pk."},
		},
		{
			// messagebird-api-token
			expression: `(?i)(?P<key>messagebird[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{25})['\"]`,
			keywords:   []string{"messagebird"},
		},
		{
			// messagebird-client-id
			expression: `(?i)(?P<key>messagebird[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-h0-9]{8}-[a-h0-9]{4}-[a-h0-9]{4}-[a-h0-9]{4}-[a-h0-9]{12})['\"]`,
			keywords:   []string{"messagebird"},
		},
		{
			// new-relic-user-api-key. `NRAK-` is the documented prefix;
			// combined with the fixed 27-char body it stays specific
			// without the surrounding quotes.
			expression: `NRAK-[A-Z0-9]{27}`,
			keywords:   []string{"NRAK-"},
		},
		{
			// new-relic-user-api-id
			expression: `(?i)(?P<key>newrelic[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[A-Z0-9]{64})['\"]`,
			keywords:   []string{"newrelic"},
		},
		{
			// new-relic-browser-api-token. `NRJS-` is the documented
			// prefix; the fixed 19-char body keeps the rule specific.
			expression: `NRJS-[a-f0-9]{19}`,
			keywords:   []string{"NRJS-"},
		},
		{
			// npm-access-token. The `npm_` prefix + fixed 36-char body is
			// unique enough that we don't anchor on surrounding quotes —
			// this catches CLI output (`npm token list`) and `.npmrc`
			// shapes (`//registry.npmjs.org/:_authToken=npm_…`) alongside
			// the JSON/YAML config form the upstream rule expected.
			expression: `npm_(?i)[a-z0-9]{36}`,
			keywords:   []string{"npm_"},
		},
		{
			// planetscale-password
			expression: `pscale_pw_(?i)[a-z0-9\-_\.]{43}`,
			keywords:   []string{"pscale_pw_"},
		},
		{
			// planetscale-api-token
			expression: `pscale_tkn_(?i)[a-z0-9\-_\.]{43}`,
			keywords:   []string{"pscale_tkn_"},
		},
		{
			// private-packagist-token
			expression: `packagist_[ou][ru]t_(?i)[a-f0-9]{68}`,
			keywords:   []string{"packagist_uut_", "packagist_ort_", "packagist_out_"},
		},
		{
			// postman-api-token
			expression: `PMAK-(?i)[a-f0-9]{24}\-[a-f0-9]{34}`,
			keywords:   []string{"PMAK-"},
		},
		{
			// pulumi-api-token
			expression: `pul-[a-f0-9]{40}`,
			keywords:   []string{"pul-"},
		},
		{
			// rubygems-api-token
			expression: `rubygems_[a-f0-9]{48}`,
			keywords:   []string{"rubygems_"},
		},
		{
			// sendgrid-api-token
			expression: `SG\.(?i)[a-z0-9_\-\.]{66}`,
			keywords:   []string{"SG."},
		},
		{
			// sendinblue-api-token
			expression: `xkeysib-[a-f0-9]{64}\-(?i)[a-z0-9]{16}`,
			keywords:   []string{"xkeysib-"},
		},
		{
			// shippo-api-token
			expression: `shippo_(live|test)_[a-f0-9]{40}`,
			keywords:   []string{"shippo_live_", "shippo_test_"},
		},
		{
			// linkedin-client-secret
			expression: `(?i)(?P<key>linkedin[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z]{16})['\"]`,
			keywords:   []string{"linkedin"},
		},
		{
			// linkedin-client-id
			expression: `(?i)(?P<key>linkedin[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{14})['\"]`,
			keywords:   []string{"linkedin"},
		},
		{
			// twitch-api-token
			expression: `(?i)(?P<key>twitch[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}['\"](?P<secret>[a-z0-9]{30})['\"]`,
			keywords:   []string{"twitch"},
		},
		{
			// typeform-api-token
			expression: `(?i)(?P<key>typeform[a-z0-9_ .\-,]{0,25})(=|>|:=|\|\|:|<=|=>|:).{0,5}(?P<secret>tfp_[a-z0-9\-_\.=]{59})`,
			keywords:   []string{"typeform"},
		},
		{
			// dockerconfig-secret
			expression: `(?i)(\.(dockerconfigjson|dockercfg):\s*\|*\s*(?P<secret>(ey|ew)+[A-Za-z0-9\/\+=]+))`,
			keywords:   []string{"dockerc"},
		},
		{
			// docker pat
			expression: `(?i)(dckr_pat_[-0-9a-zA-Z]{27})`,
			keywords:   []string{"dckr_pat"},
		},

		// --- Patterns added on top of the upstream Trivy / mcp-gateway
		// catalogue. Each one targets a credential format whose prefix
		// is unique enough to keep the keyword pre-filter cheap and the
		// regex's false-positive rate low.

		{
			// openai-api-key. Every modern OpenAI key (project keys
			// `sk-proj-…`, service-account keys `sk-svcacct-…`, admin
			// keys `sk-admin-…`, and the original `sk-…` keys reissued
			// after May 2024) embeds the literal substring "T3BlbkFJ"
			// (base64 for "OpenAI") between two long alphanumeric runs.
			// That marker keeps both the keyword filter and the regex
			// extremely specific.
			expression: `sk-[A-Za-z0-9_-]{20,}T3BlbkFJ[A-Za-z0-9_-]{20,}`,
			keywords:   []string{"T3BlbkFJ"},
		},
		{
			// anthropic-api-key. Claude keys follow
			// `sk-ant-(api|sid)NN-<base64url>` and are ~108 chars long;
			// the trailing "AA" is the standard base64 padding.
			expression: `sk-ant-(api|sid)\d{2}-[A-Za-z0-9_-]{93}AA`,
			keywords:   []string{"sk-ant-"},
		},
		{
			// google-api-key. Used by Maps, Cloud, Firebase, Gemini and
			// most other Google REST APIs. The `AIza` prefix is fixed.
			expression: `AIza[0-9A-Za-z_-]{35}`,
			keywords:   []string{"AIza"},
		},
		{
			// google-oauth-client-secret. Issued in the Google Cloud
			// Console for OAuth 2.0 clients; always 35 chars total.
			expression: `GOCSPX-[A-Za-z0-9_-]{28}`,
			keywords:   []string{"GOCSPX-"},
		},
		{
			// digitalocean-token. v1 personal-access tokens (`dop_v1_`),
			// OAuth tokens (`doo_v1_`), and OAuth refresh tokens
			// (`dor_v1_`) all share the 71-char total shape: 7-char
			// prefix + 64 lowercase hex.
			expression: `do[opr]_v1_[a-f0-9]{64}`,
			keywords:   []string{"dop_v1_", "doo_v1_", "dor_v1_"},
		},
		{
			// stripe-webhook-signing-secret. Used to verify incoming
			// webhook payloads; leakage lets attackers forge events.
			expression: `whsec_[A-Za-z0-9]{32,}`,
			keywords:   []string{"whsec_"},
		},
		{
			// jfrog-artifactory-api-key. Distinct from access tokens;
			// the `AKCp` prefix is documented and the body is between
			// 69 and 73 alphanumeric characters depending on when the
			// key was issued.
			expression: `AKCp[A-Za-z0-9]{69,73}`,
			keywords:   []string{"AKCp"},
		},
		{
			// tencent-cloud-secret-id. Tencent's analogue of an AWS
			// access-key-id, used by the COS / CVM / etc. APIs.
			expression: `AKID[A-Za-z0-9]{32}`,
			keywords:   []string{"AKID"},
		},
		{
			// sentry-user-auth-token. The `sntrys_` prefix is followed
			// by a base64url-encoded JWT-style payload that always
			// starts with `eyJ` (the base64 of `{"`).
			expression: `sntrys_eyJ[A-Za-z0-9+/=_-]{40,}`,
			keywords:   []string{"sntrys_"},
		},
		{
			// stripe-restricted-key. Restricted API keys (introduced
			// alongside the publishable / secret keys) follow the same
			// `<prefix>_<env>_<body>` shape as `pk_` / `sk_`. Leakage of
			// a restricted key still grants the scoped Stripe permissions
			// it was issued with, so it must be redacted.
			expression: `(?i)rk_(test|live)_[0-9a-z]{10,32}`,
			keywords:   []string{"rk_test_", "rk_live_"},
		},
		{
			// notion-integration-token. The `ntn_` prefix is the modern
			// (post-2023) format for internal-integration tokens; the
			// 46-character body is fixed.
			expression: `ntn_[A-Za-z0-9]{46}`,
			keywords:   []string{"ntn_"},
		},
		{
			// gitlab-pipeline-trigger-token. `glptt-` is the documented
			// prefix for trigger tokens; body is 40 lowercase hex.
			expression: `glptt-[a-f0-9]{40}`,
			keywords:   []string{"glptt-"},
		},
		{
			// vault-service-token. HashiCorp Vault service tokens issued
			// by recent Vault versions carry the `hvs.` prefix and a
			// base64url body whose length varies with the policies /
			// metadata encoded inside the CBOR payload. The lower bound
			// of 90 chars covers a default-policy token; the upper bound
			// of 200 covers tokens carrying multiple policies and
			// namespace metadata (matching the looser bound Trivy uses
			// for similar Vault formats).
			expression: `hvs\.[A-Za-z0-9_-]{90,200}`,
			keywords:   []string{"hvs."},
		},
		{
			// slack-rotating-token. Modern Slack OAuth issues refresh
			// tokens (`xoxe-…`) and rotating bot/user tokens
			// (`xoxe.xoxb-…` / `xoxe.xoxp-…`) whose bodies include dashes
			// and dots — a shape the legacy `slack-access-token` rule
			// (locked to `xox[baprs]-`) does not cover.
			//
			// The body class `[A-Za-z0-9.-]` happens to overlap with
			// neighbouring URL / hostname text (e.g. `api.slack.com`)
			// so we cap the quantifier at 300 — comfortably above the
			// longest observed Slack rotating-token body — to keep an
			// adjacent dotted identifier from being swallowed into the
			// redaction span when the token isn't separated from it by
			// whitespace or punctuation.
			expression: `xoxe(\.xox[bp])?-[A-Za-z0-9.-]{40,300}`,
			keywords:   []string{"xoxe-", "xoxe.xox"},
		},
		{
			// replicate-api-token. Replicate keys carry the `r8_`
			// prefix; the fixed 37-char body keeps the rule specific.
			expression: `r8_[A-Za-z0-9]{37}`,
			keywords:   []string{"r8_"},
		},
		{
			// square-access-token. Square production / sandbox access
			// tokens carry the `EAAA` prefix and a 60-character body.
			expression: `EAAA[A-Za-z0-9_-]{60}`,
			keywords:   []string{"EAAA"},
		},
		{
			// atlassian-api-token (Cloud). Atlassian Cloud API tokens
			// carry the very distinctive `ATATT3xFfGF0` prefix followed
			// by a long base64url body and an 8-char hex CRC. The
			// existing `atlassian-api-token` rule only catches values
			// preceded by an `atlassian` keyword — this rule fills the
			// gap for bare leakage in CLI output / logs.
			expression: `ATATT3xFfGF0[A-Za-z0-9_=-]{180,250}`,
			keywords:   []string{"ATATT3xFfGF0"},
		},
	}
})

// compiledRule is the runtime form of a [rule]: its keywords are
// folded into a [kwMask] over the catalogue's shared keyword index,
// and its regex is compiled lazily on first match. A clean input —
// the overwhelmingly common case — therefore never pays for
// compiling any of the ~85 expressions in the catalogue, only for
// the keyword bitset and the AC table.
type compiledRule struct {
	kwBits  kwMask
	compile func() (*regexp.Regexp, int) // memoised; returns (re, secretIdx)
}

// ruleSet bundles the runtime catalogue with the Aho–Corasick
// pre-filter built from its (de-duplicated) keyword set. They live
// behind a single [sync.OnceValue] so neither piece of state is
// constructed until the first scan that actually needs it; per-rule
// regex compilation is further deferred via
// [compiledRule.compile].
type ruleSet struct {
	rules []compiledRule
	ac    *acAutomaton
}

var compiledRuleSet = sync.OnceValue(func() *ruleSet {
	src := rules()

	// Assign an index to every distinct (lower-cased) keyword. Rules
	// share keywords like "discord" or "bitbucket" — deduplication
	// means the AC scans for them once and each rule's bitset refers
	// to the shared slot.
	kwIdx := make(map[string]int)
	var uniq []string
	for _, r := range src {
		for _, k := range r.keywords {
			lk := strings.ToLower(k)
			if _, ok := kwIdx[lk]; !ok {
				kwIdx[lk] = len(uniq)
				uniq = append(uniq, lk)
			}
		}
	}

	rules := make([]compiledRule, len(src))
	for i, r := range src {
		var bits kwMask
		for _, k := range r.keywords {
			bits.set(kwIdx[strings.ToLower(k)])
		}
		expr := r.expression // bind for the closure below
		rules[i] = compiledRule{
			kwBits: bits,
			compile: sync.OnceValues(func() (*regexp.Regexp, int) {
				re := regexp.MustCompile(expr)
				return re, re.SubexpIndex("secret")
			}),
		}
	}
	return &ruleSet{rules: rules, ac: buildAhoCorasick(uniq)}
})
