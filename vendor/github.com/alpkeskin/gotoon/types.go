package gotoon

// EncodeOptions represents the options for encoding values to TOON format
type EncodeOptions struct {
	// Indent is the number of spaces per indentation level (default: 2)
	Indent int

	// Delimiter is the delimiter to use for array values and tabular rows
	// Valid values: "," (comma), "\t" (tab), "|" (pipe)
	// Default: ","
	Delimiter string

	// LengthMarker when true adds "#" prefix to array lengths (e.g., [#3] instead of [3])
	// Default: false
	LengthMarker bool
}

// EncodeOption is a function that modifies EncodeOptions
type EncodeOption func(*EncodeOptions)

// WithIndent sets the number of spaces per indentation level
func WithIndent(n int) EncodeOption {
	return func(opts *EncodeOptions) {
		opts.Indent = n
	}
}

// WithDelimiter sets the delimiter for array values and tabular rows
func WithDelimiter(d string) EncodeOption {
	return func(opts *EncodeOptions) {
		opts.Delimiter = d
	}
}

// WithLengthMarker enables the length marker prefix for arrays
func WithLengthMarker() EncodeOption {
	return func(opts *EncodeOptions) {
		opts.LengthMarker = true
	}
}

// defaultOptions returns the default encoding options
func defaultOptions() *EncodeOptions {
	return &EncodeOptions{
		Indent:       2,
		Delimiter:    DefaultDelimiter,
		LengthMarker: false,
	}
}

// resolveOptions applies the given options to the default options
func resolveOptions(opts []EncodeOption) *EncodeOptions {
	options := defaultOptions()
	for _, opt := range opts {
		opt(options)
	}
	return options
}
