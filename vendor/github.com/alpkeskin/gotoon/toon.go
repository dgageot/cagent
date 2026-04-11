// Package gotoon provides encoding for Token-Oriented Object Notation (TOON),
// a compact, human-readable format designed for passing structured data to
// Large Language Models with significantly reduced token usage.
//
// TOON is optimized for uniform complex objects and provides 30-60% token
// reduction compared to JSON while maintaining high LLM comprehension accuracy.
//
// Example usage:
//
//	data := map[string]interface{}{
//		"users": []map[string]interface{}{
//			{"id": 1, "name": "Alice", "role": "admin"},
//			{"id": 2, "name": "Bob", "role": "user"},
//		},
//	}
//
//	encoded, err := gotoon.Encode(data)
//	if err != nil {
//		log.Fatal(err)
//	}
//	fmt.Println(encoded)
//	// Output:
//	// users[2]{id,name,role}:
//	//   1,Alice,admin
//	//   2,Bob,user
package gotoon

// Encode converts any Go value to TOON format string.
//
// The input value is normalized to a JSON-compatible representation:
//   - Primitives (bool, int, float, string) are encoded as-is
//   - Structs are converted to maps using exported fields (respects json tags)
//   - Slices and arrays remain as arrays
//   - Maps with string keys remain as objects
//   - time.Time is converted to RFC3339Nano format
//   - NaN and Infinity become null
//   - Nil, undefined, functions become null
//
// Options can be provided to customize the encoding:
//   - WithIndent(n): Set indentation size (default: 2 spaces)
//   - WithDelimiter(d): Set delimiter for arrays ("," | "\t" | "|", default: ",")
//   - WithLengthMarker(): Add "#" prefix to array lengths (e.g., [#3])
//
// Example with options:
//
//	encoded, err := gotoon.Encode(data,
//		gotoon.WithIndent(4),
//		gotoon.WithDelimiter("\t"),
//		gotoon.WithLengthMarker(),
//	)
func Encode(input interface{}, opts ...EncodeOption) (string, error) {
	// Normalize the input value
	normalized := normalizeValue(input)

	// Resolve options
	options := resolveOptions(opts)

	// Encode the normalized value
	result := encodeValue(normalized, options)

	return result, nil
}
