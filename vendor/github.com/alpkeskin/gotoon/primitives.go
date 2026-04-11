package gotoon

import (
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

// encodePrimitive encodes a primitive value (string, number, bool, null)
func encodePrimitive(value interface{}, delimiter string) string {
	if value == nil {
		return NullLiteral
	}

	switch v := value.(type) {
	case bool:
		if v {
			return TrueLiteral
		}
		return FalseLiteral

	case float64:
		// Format number without scientific notation
		return formatNumber(v)

	case string:
		return encodeStringLiteral(v, delimiter)

	default:
		return NullLiteral
	}
}

// formatNumber formats a float64 without scientific notation
func formatNumber(f float64) string {
	// Check if it's an integer
	if f == float64(int64(f)) {
		return strconv.FormatInt(int64(f), 10)
	}
	// Use decimal format with appropriate precision
	return strconv.FormatFloat(f, 'f', -1, 64)
}

// encodeStringLiteral encodes a string, adding quotes if necessary
func encodeStringLiteral(value string, delimiter string) string {
	if isSafeUnquoted(value, delimiter) {
		return value
	}
	return DoubleQuote + escapeString(value) + DoubleQuote
}

// escapeString escapes special characters in a string
func escapeString(value string) string {
	value = strings.ReplaceAll(value, Backslash, Backslash+Backslash)
	value = strings.ReplaceAll(value, DoubleQuote, Backslash+DoubleQuote)
	value = strings.ReplaceAll(value, "\n", Backslash+"n")
	value = strings.ReplaceAll(value, "\r", Backslash+"r")
	value = strings.ReplaceAll(value, "\t", Backslash+"t")
	return value
}

// isSafeUnquoted checks if a string can be safely represented without quotes
func isSafeUnquoted(value string, delimiter string) bool {
	if value == "" {
		return false
	}

	// Check for leading/trailing whitespace
	if strings.TrimSpace(value) != value {
		return false
	}

	// Check for reserved literals
	if value == TrueLiteral || value == FalseLiteral || value == NullLiteral {
		return false
	}

	// Check if it looks like a number
	if isNumericLike(value) {
		return false
	}

	// Check for colon (always structural)
	if strings.Contains(value, Colon) {
		return false
	}

	// Check for quotes and backslash
	if strings.Contains(value, DoubleQuote) || strings.Contains(value, Backslash) {
		return false
	}

	// Check for brackets and braces
	if strings.ContainsAny(value, "[]{}") {
		return false
	}

	// Check for control characters
	if strings.ContainsAny(value, "\n\r\t") {
		return false
	}

	// Check for the active delimiter
	if strings.Contains(value, delimiter) {
		return false
	}

	// Check for hyphen at start (list marker)
	if strings.HasPrefix(value, ListItemMarker) {
		return false
	}

	return true
}

var numericPattern = regexp.MustCompile(`^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$|^0\d+$`)

// isNumericLike checks if a string looks like a number
func isNumericLike(value string) bool {
	return numericPattern.MatchString(value)
}

// encodeKey encodes an object key, adding quotes if necessary
func encodeKey(key string) string {
	if isValidUnquotedKey(key) {
		return key
	}
	return DoubleQuote + escapeString(key) + DoubleQuote
}

var validKeyPattern = regexp.MustCompile(`^[A-Za-z_][\w.]*$`)

// isValidUnquotedKey checks if a key can be used without quotes
func isValidUnquotedKey(key string) bool {
	return validKeyPattern.MatchString(key)
}

// joinEncodedValues joins multiple primitive values with a delimiter
func joinEncodedValues(values []interface{}, delimiter string) string {
	encoded := make([]string, len(values))
	for i, v := range values {
		encoded[i] = encodePrimitive(v, delimiter)
	}
	return strings.Join(encoded, delimiter)
}

// formatHeader formats an array or table header
func formatHeader(length int, options headerOptions) string {
	var sb strings.Builder

	if options.key != "" {
		sb.WriteString(encodeKey(options.key))
	}

	// Array length with optional marker
	sb.WriteString(OpenBracket)
	if options.lengthMarker {
		sb.WriteString("#")
	}
	sb.WriteString(strconv.Itoa(length))

	// Include delimiter if it's not the default (comma)
	if options.delimiter != DefaultDelimiter {
		sb.WriteString(options.delimiter)
	}

	sb.WriteString(CloseBracket)

	// Field list for tabular format
	if len(options.fields) > 0 {
		sb.WriteString(OpenBrace)
		quotedFields := make([]string, len(options.fields))
		for i, field := range options.fields {
			quotedFields[i] = encodeKey(field)
		}
		sb.WriteString(strings.Join(quotedFields, options.delimiter))
		sb.WriteString(CloseBrace)
	}

	sb.WriteString(Colon)

	return sb.String()
}

// headerOptions holds options for formatting headers
type headerOptions struct {
	key          string
	fields       []string
	delimiter    string
	lengthMarker bool
}

// formatInlineArray formats a primitive array as an inline string
func formatInlineArray(values []interface{}, delimiter string, prefix string, lengthMarker bool) string {
	header := formatHeader(len(values), headerOptions{
		key:          prefix,
		delimiter:    delimiter,
		lengthMarker: lengthMarker,
	})

	if len(values) == 0 {
		return header
	}

	joined := joinEncodedValues(values, delimiter)
	return fmt.Sprintf("%s %s", header, joined)
}
