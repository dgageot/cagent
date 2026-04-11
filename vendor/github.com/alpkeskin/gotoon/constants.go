package gotoon

// List markers
const (
	ListItemMarker = "-"
	ListItemPrefix = "- "
)

// Structural characters
const (
	Comma = ","
	Colon = ":"
	Space = " "
	Pipe  = "|"
	Tab   = "\t"
)

// Brackets and braces
const (
	OpenBracket  = "["
	CloseBracket = "]"
	OpenBrace    = "{"
	CloseBrace   = "}"
)

// Literals
const (
	NullLiteral  = "null"
	TrueLiteral  = "true"
	FalseLiteral = "false"
)

// Escape characters
const (
	Backslash      = "\\"
	DoubleQuote    = "\""
	Newline        = "\n"
	CarriageReturn = "\r"
)

// Delimiters
const (
	DelimiterComma = ","
	DelimiterTab   = "\t"
	DelimiterPipe  = "|"
)

// DefaultDelimiter is the default delimiter for arrays and tabular data
const DefaultDelimiter = DelimiterComma
