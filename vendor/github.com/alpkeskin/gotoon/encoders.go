package gotoon

import (
	"fmt"
	"sort"
)

// encodeValue encodes a normalized value to TOON format
func encodeValue(value interface{}, opts *EncodeOptions) string {
	if isPrimitive(value) {
		return encodePrimitive(value, opts.Delimiter)
	}

	writer := NewLineWriter(opts.Indent)

	if arr, ok := value.([]interface{}); ok {
		encodeArray("", arr, writer, 0, opts)
	} else if obj, ok := value.(map[string]interface{}); ok {
		encodeObject(obj, writer, 0, opts)
	}

	return writer.String()
}

// encodeObject encodes an object (map) to TOON format
func encodeObject(obj map[string]interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	// Sort keys for deterministic output
	keys := make([]string, 0, len(obj))
	for k := range obj {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	for _, key := range keys {
		encodeKeyValuePair(key, obj[key], writer, depth, opts)
	}
}

// encodeKeyValuePair encodes a single key-value pair
func encodeKeyValuePair(key string, value interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	encodedKey := encodeKey(key)

	if isPrimitive(value) {
		writer.Push(depth, fmt.Sprintf("%s: %s", encodedKey, encodePrimitive(value, opts.Delimiter)))
	} else if arr, ok := value.([]interface{}); ok {
		encodeArray(key, arr, writer, depth, opts)
	} else if obj, ok := value.(map[string]interface{}); ok {
		keys := make([]string, 0, len(obj))
		for k := range obj {
			keys = append(keys, k)
		}

		if len(keys) == 0 {
			// Empty object
			writer.Push(depth, encodedKey+Colon)
		} else {
			writer.Push(depth, encodedKey+Colon)
			encodeObject(obj, writer, depth+1, opts)
		}
	}
}

// encodeArray encodes an array with various strategies based on content
func encodeArray(key string, arr []interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	if len(arr) == 0 {
		header := formatHeader(0, headerOptions{
			key:          key,
			delimiter:    opts.Delimiter,
			lengthMarker: opts.LengthMarker,
		})
		writer.Push(depth, header)
		return
	}

	// Strategy 1: Primitive array (inline)
	if isArrayOfPrimitives(arr) {
		encodeInlinePrimitiveArray(key, arr, writer, depth, opts)
		return
	}

	// Strategy 2: Array of arrays (all primitives)
	if isArrayOfArrays(arr) {
		allPrimitiveArrays := true
		for _, item := range arr {
			if itemArr, ok := item.([]interface{}); ok {
				if !isArrayOfPrimitives(itemArr) {
					allPrimitiveArrays = false
					break
				}
			}
		}
		if allPrimitiveArrays {
			encodeArrayOfArraysAsListItems(key, arr, writer, depth, opts)
			return
		}
	}

	// Strategy 3: Array of objects (try tabular format)
	if isArrayOfObjects(arr) {
		objects := make([]map[string]interface{}, len(arr))
		for i, item := range arr {
			objects[i] = item.(map[string]interface{})
		}

		header := detectTabularHeader(objects)
		if header != nil {
			encodeArrayOfObjectsAsTabular(key, objects, header, writer, depth, opts)
		} else {
			encodeMixedArrayAsListItems(key, arr, writer, depth, opts)
		}
		return
	}

	// Strategy 4: Mixed array (fallback to list format)
	encodeMixedArrayAsListItems(key, arr, writer, depth, opts)
}

// encodeInlinePrimitiveArray encodes a primitive array in inline format
func encodeInlinePrimitiveArray(prefix string, values []interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	formatted := formatInlineArray(values, opts.Delimiter, prefix, opts.LengthMarker)
	writer.Push(depth, formatted)
}

// encodeArrayOfArraysAsListItems encodes an array of primitive arrays in list format
func encodeArrayOfArraysAsListItems(prefix string, arrays []interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	header := formatHeader(len(arrays), headerOptions{
		key:          prefix,
		delimiter:    opts.Delimiter,
		lengthMarker: opts.LengthMarker,
	})
	writer.Push(depth, header)

	for _, item := range arrays {
		if arr, ok := item.([]interface{}); ok && isArrayOfPrimitives(arr) {
			inline := formatInlineArray(arr, opts.Delimiter, "", opts.LengthMarker)
			writer.Push(depth+1, ListItemPrefix+inline)
		}
	}
}

// detectTabularHeader detects if an array of objects can use tabular format
func detectTabularHeader(objects []map[string]interface{}) []string {
	if len(objects) == 0 {
		return nil
	}

	// Get keys from first object
	firstObj := objects[0]
	if len(firstObj) == 0 {
		return nil
	}

	// Extract and sort keys for deterministic output
	firstKeys := make([]string, 0, len(firstObj))
	for k := range firstObj {
		firstKeys = append(firstKeys, k)
	}
	sort.Strings(firstKeys)

	// Check if all objects have the same keys with primitive values
	if isTabularArray(objects, firstKeys) {
		return firstKeys
	}

	return nil
}

// isTabularArray checks if all objects have the same keys and only primitive values
func isTabularArray(objects []map[string]interface{}, header []string) bool {
	for _, obj := range objects {
		// All objects must have the same number of keys
		if len(obj) != len(header) {
			return false
		}

		// Check that all header keys exist and values are primitives
		for _, key := range header {
			value, exists := obj[key]
			if !exists {
				return false
			}
			if !isPrimitive(value) {
				return false
			}
		}
	}

	return true
}

// encodeArrayOfObjectsAsTabular encodes an array of uniform objects in tabular format
func encodeArrayOfObjectsAsTabular(prefix string, objects []map[string]interface{}, header []string, writer *LineWriter, depth int, opts *EncodeOptions) {
	headerStr := formatHeader(len(objects), headerOptions{
		key:          prefix,
		fields:       header,
		delimiter:    opts.Delimiter,
		lengthMarker: opts.LengthMarker,
	})
	writer.Push(depth, headerStr)

	writeTabularRows(objects, header, writer, depth+1, opts)
}

// writeTabularRows writes the data rows for a tabular array
func writeTabularRows(objects []map[string]interface{}, header []string, writer *LineWriter, depth int, opts *EncodeOptions) {
	for _, obj := range objects {
		values := make([]interface{}, len(header))
		for i, key := range header {
			values[i] = obj[key]
		}
		joined := joinEncodedValues(values, opts.Delimiter)
		writer.Push(depth, joined)
	}
}

// encodeMixedArrayAsListItems encodes a mixed array in list format
func encodeMixedArrayAsListItems(prefix string, items []interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	header := formatHeader(len(items), headerOptions{
		key:          prefix,
		delimiter:    opts.Delimiter,
		lengthMarker: opts.LengthMarker,
	})
	writer.Push(depth, header)

	for _, item := range items {
		if isPrimitive(item) {
			// Direct primitive as list item
			writer.Push(depth+1, ListItemPrefix+encodePrimitive(item, opts.Delimiter))
		} else if arr, ok := item.([]interface{}); ok {
			// Direct array as list item
			if isArrayOfPrimitives(arr) {
				inline := formatInlineArray(arr, opts.Delimiter, "", opts.LengthMarker)
				writer.Push(depth+1, ListItemPrefix+inline)
			}
		} else if obj, ok := item.(map[string]interface{}); ok {
			// Object as list item
			encodeObjectAsListItem(obj, writer, depth+1, opts)
		}
	}
}

// encodeObjectAsListItem encodes an object as a list item
func encodeObjectAsListItem(obj map[string]interface{}, writer *LineWriter, depth int, opts *EncodeOptions) {
	// Sort keys for deterministic output
	keys := make([]string, 0, len(obj))
	for k := range obj {
		keys = append(keys, k)
	}
	sort.Strings(keys)

	if len(keys) == 0 {
		writer.Push(depth, ListItemMarker)
		return
	}

	// First key-value on the same line as "- "
	firstKey := keys[0]
	encodedKey := encodeKey(firstKey)
	firstValue := obj[firstKey]

	if isPrimitive(firstValue) {
		writer.Push(depth, fmt.Sprintf("%s%s: %s", ListItemPrefix, encodedKey, encodePrimitive(firstValue, opts.Delimiter)))
	} else if arr, ok := firstValue.([]interface{}); ok {
		if isArrayOfPrimitives(arr) {
			// Inline format for primitive arrays
			formatted := formatInlineArray(arr, opts.Delimiter, firstKey, opts.LengthMarker)
			writer.Push(depth, ListItemPrefix+formatted)
		} else if isArrayOfObjects(arr) {
			// Check if array of objects can use tabular format
			objects := make([]map[string]interface{}, len(arr))
			for i, item := range arr {
				objects[i] = item.(map[string]interface{})
			}

			header := detectTabularHeader(objects)
			if header != nil {
				// Tabular format
				headerStr := formatHeader(len(arr), headerOptions{
					key:          firstKey,
					fields:       header,
					delimiter:    opts.Delimiter,
					lengthMarker: opts.LengthMarker,
				})
				writer.Push(depth, ListItemPrefix+headerStr)
				writeTabularRows(objects, header, writer, depth+1, opts)
			} else {
				// Fall back to list format
				writer.Push(depth, fmt.Sprintf("%s%s[%d]:", ListItemPrefix, encodedKey, len(arr)))
				for _, item := range arr {
					if itemObj, ok := item.(map[string]interface{}); ok {
						encodeObjectAsListItem(itemObj, writer, depth+1, opts)
					}
				}
			}
		} else {
			// Complex arrays on separate lines
			writer.Push(depth, fmt.Sprintf("%s%s[%d]:", ListItemPrefix, encodedKey, len(arr)))

			for _, item := range arr {
				if isPrimitive(item) {
					writer.Push(depth+1, ListItemPrefix+encodePrimitive(item, opts.Delimiter))
				} else if itemArr, ok := item.([]interface{}); ok && isArrayOfPrimitives(itemArr) {
					inline := formatInlineArray(itemArr, opts.Delimiter, "", opts.LengthMarker)
					writer.Push(depth+1, ListItemPrefix+inline)
				} else if itemObj, ok := item.(map[string]interface{}); ok {
					encodeObjectAsListItem(itemObj, writer, depth+1, opts)
				}
			}
		}
	} else if nestedObj, ok := firstValue.(map[string]interface{}); ok {
		nestedKeys := make([]string, 0, len(nestedObj))
		for k := range nestedObj {
			nestedKeys = append(nestedKeys, k)
		}

		if len(nestedKeys) == 0 {
			writer.Push(depth, ListItemPrefix+encodedKey+Colon)
		} else {
			writer.Push(depth, ListItemPrefix+encodedKey+Colon)
			encodeObject(nestedObj, writer, depth+2, opts)
		}
	}

	// Remaining keys on indented lines
	for i := 1; i < len(keys); i++ {
		key := keys[i]
		encodeKeyValuePair(key, obj[key], writer, depth+1, opts)
	}
}
