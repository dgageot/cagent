# GoTOON - Token-Oriented Object Notation for Go

[![CI](https://github.com/alpkeskin/gotoon/actions/workflows/ci.yml/badge.svg)](https://github.com/alpkeskin/gotoon/actions/workflows/ci.yml)
[![Go Report Card](https://goreportcard.com/badge/github.com/alpkeskin/gotoon)](https://goreportcard.com/report/github.com/alpkeskin/gotoon)
[![codecov](https://codecov.io/gh/alpkeskin/gotoon/branch/main/graph/badge.svg)](https://codecov.io/gh/alpkeskin/gotoon)
[![Go Reference](https://pkg.go.dev/badge/github.com/alpkeskin/gotoon.svg)](https://pkg.go.dev/github.com/alpkeskin/gotoon)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Go Version](https://img.shields.io/github/go-mod/go-version/alpkeskin/gotoon)](https://go.dev/)

**GoTOON** is a Go implementation of [TOON (Token-Oriented Object Notation)](https://github.com/johannschopplich/toon), a compact, human-readable format designed for passing structured data to Large Language Models with significantly reduced token usage.

TOON excels at **uniform complex objects** – multiple fields per row, same structure across items. It achieves **30-60% token reduction** compared to JSON while maintaining high LLM comprehension accuracy.

![Toon](/.github/og.png)

## Why TOON?

LLM tokens cost money, and standard JSON is verbose and token-expensive:

```json
{
  "users": [
    { "id": 1, "name": "Alice", "role": "admin" },
    { "id": 2, "name": "Bob", "role": "user" }
  ]
}
```

TOON conveys the same information with **fewer tokens**:

```
users[2]{id,name,role}:
  1,Alice,admin
  2,Bob,user
```

## Key Features

- 💸 **Token-efficient:** typically 30–60% fewer tokens than JSON
- 🤿 **LLM-friendly guardrails:** explicit lengths and field lists help models validate output
- 🍱 **Minimal syntax:** removes redundant punctuation (braces, brackets, most quotes)
- 📐 **Indentation-based structure:** replaces braces with whitespace for better readability
- 🧺 **Tabular arrays:** declare keys once, then stream rows without repetition
- 🛠️ **Go-idiomatic API:** clean, simple interface with functional options

## Installation

```bash
go get github.com/alpkeskin/gotoon
```

## Quick Start

```go
package main

import (
    "fmt"
    "log"

    "github.com/alpkeskin/gotoon"
)

func main() {
    data := map[string]interface{}{
        "users": []map[string]interface{}{
            {"id": 1, "name": "Alice", "role": "admin"},
            {"id": 2, "name": "Bob", "role": "user"},
        },
    }

    encoded, err := gotoon.Encode(data)
    if err != nil {
        log.Fatal(err)
    }

    fmt.Println(encoded)
}
```

Output:

```
users[2]{id,name,role}:
  1,Alice,admin
  2,Bob,user
```

## API

### `Encode(input interface{}, opts ...EncodeOption) (string, error)`

Converts any Go value to TOON format string.

**Input normalization:**
- Primitives (bool, int, float, string) are encoded as-is
- Structs are converted to maps using exported fields (respects `json` tags)
- Slices and arrays remain as arrays
- Maps with string keys remain as objects
- `time.Time` is converted to RFC3339Nano format
- `NaN` and `Infinity` become `null`
- `nil`, functions become `null`

**Example:**

```go
type User struct {
    ID   int    `json:"id"`
    Name string `json:"name"`
    Role string `json:"role"`
}

users := []User{
    {ID: 1, Name: "Alice", Role: "admin"},
    {ID: 2, Name: "Bob", Role: "user"},
}

encoded, _ := gotoon.Encode(map[string]interface{}{"users": users})
// Output:
// users[2]{id,name,role}:
//   1,Alice,admin
//   2,Bob,user
```

### Encoding Options

GoTOON supports functional options for customization:

#### `WithIndent(n int)`

Sets the number of spaces per indentation level (default: 2).

```go
gotoon.Encode(data, gotoon.WithIndent(4))
```

#### `WithDelimiter(d string)`

Sets the delimiter for array values and tabular rows. Valid values: `","` (comma, default), `"\t"` (tab), `"|"` (pipe).

```go
// Using tab delimiter
gotoon.Encode(data, gotoon.WithDelimiter("\t"))
// Output:
// users[2	]{id	name	role}:
//   1	Alice	admin
//   2	Bob	user
```

#### `WithLengthMarker()`

Adds `#` prefix to array lengths for clarity (e.g., `[#3]` instead of `[3]`).

```go
gotoon.Encode(data, gotoon.WithLengthMarker())
// Output:
// users[#2]{id,name,role}:
//   1,Alice,admin
//   2,Bob,user
```

### Combining Options

```go
encoded, _ := gotoon.Encode(data,
    gotoon.WithIndent(4),
    gotoon.WithDelimiter("\t"),
    gotoon.WithLengthMarker(),
)
```

## Format Overview

### Objects

Simple objects with primitive values:

```go
data := map[string]interface{}{
    "id":     123,
    "name":   "Ada",
    "active": true,
}
// Output:
// id: 123
// name: Ada
// active: true
```

Nested objects:

```go
data := map[string]interface{}{
    "user": map[string]interface{}{
        "id":   123,
        "name": "Ada",
    },
}
// Output:
// user:
//   id: 123
//   name: Ada
```

### Arrays

#### Primitive Arrays (Inline)

```go
data := map[string]interface{}{
    "tags": []string{"admin", "ops", "dev"},
}
// Output:
// tags[3]: admin,ops,dev
```

#### Arrays of Objects (Tabular)

When all objects share the same primitive fields, TOON uses an efficient **tabular format**:

```go
data := map[string]interface{}{
    "items": []map[string]interface{}{
        {"sku": "A1", "qty": 2, "price": 9.99},
        {"sku": "B2", "qty": 1, "price": 14.5},
    },
}
// Output:
// items[2]{price,qty,sku}:
//   9.99,2,A1
//   14.5,1,B2
```

#### Mixed and Non-Uniform Arrays

Arrays that don't meet tabular requirements use list format:

```go
data := map[string]interface{}{
    "items": []interface{}{
        1,
        "text",
        map[string]interface{}{"key": "value"},
    },
}
// Output:
// items[3]:
//   - 1
//   - text
//   - key: value
```

### Quoting Rules

TOON quotes strings **only when necessary** to maximize token efficiency:

- Empty strings: `""`
- Contains delimiter, colon, quotes, or control chars: `"hello, world"`
- Leading/trailing spaces: `" padded "`
- Looks like boolean/number/null: `"true"`, `"42"`
- Unicode and emoji are safe unquoted: `hello 👋 world`

## Examples

See the [examples/basic](examples/basic) directory for more comprehensive examples including:

- Simple objects
- Tabular arrays
- Nested structures
- Primitive arrays
- Using structs with JSON tags
- Custom delimiters
- Mixed arrays
- Time values
- E-commerce orders

Run the examples:

```bash
cd examples/basic
go run main.go
```

## Testing

```bash
go test -v
```

## Benchmarks

Based on the original TOON benchmarks using GPT-5's tokenizer:

- **GitHub Repositories (100 repos):** 42.3% token reduction vs JSON
- **Daily Analytics (180 days):** 58.9% token reduction vs JSON
- **E-Commerce Order:** 35.4% token reduction vs JSON

**Overall:** 49.1% token reduction vs JSON across all benchmarks

## Comparison with JSON

**JSON** (257 tokens):
```json
{
  "order": {
    "id": "ORD-12345",
    "customer": {
      "name": "John Doe",
      "email": "john@example.com"
    },
    "items": [
      { "sku": "WIDGET-1", "quantity": 2, "price": 19.99 },
      { "sku": "GADGET-2", "quantity": 1, "price": 49.99 }
    ],
    "total": 89.97
  }
}
```

**TOON** (166 tokens - 35.4% reduction):
```
order:
  customer:
    email: john@example.com
    name: John Doe
  id: ORD-12345
  items[2]{price,quantity,sku}:
    19.99,2,WIDGET-1
    49.99,1,GADGET-2
  total: 89.97
```

## Project Structure

```
gotoon/
├── go.mod              # Go module definition
├── README.md           # This file
├── toon.go             # Public API (Encode function)
├── types.go            # Options and type definitions
├── constants.go        # String constants and delimiters
├── normalize.go        # Value normalization and type guards
├── writer.go           # LineWriter implementation
├── primitives.go       # Primitive encoding and quoting
├── encoders.go         # Core encoding logic
├── toon_test.go        # Unit tests
└── examples/
    └── basic/
        └── main.go     # Example usage
```

## Implementation Notes

- **Deterministic output:** Map keys are sorted alphabetically for consistent encoding
- **Reflection-based normalization:** Automatically converts structs, slices, and maps
- **Efficient string building:** Uses `strings.Builder` for performance
- **Type-safe options:** Functional options pattern for clean API
- **Comprehensive testing:** Full test coverage with table-driven tests

## Using TOON with LLMs

TOON works best when you show the format instead of describing it. The structure is self-documenting – models parse it naturally once they see the pattern.

### Sending TOON to LLMs (Input)

Wrap your encoded data in a fenced code block:

````markdown
```toon
users[3]{id,name,role}:
  1,Alice,admin
  2,Bob,user
  3,Charlie,user
```
````

### Generating TOON from LLMs (Output)

For output, be more explicit:

```
Data is in TOON format (2-space indent, arrays show length and fields).

Task: Return only users with role "user" as TOON. Use the same header.
Set [N] to match the row count. Output only the code block.
```

## Credits

GoTOON is a Go port of the original [TOON format](https://github.com/johannschopplich/toon) created by [Johann Schopplich](https://github.com/johannschopplich).
