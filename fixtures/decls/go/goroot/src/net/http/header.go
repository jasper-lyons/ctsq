// Copyright notice.

package http

import (
	"io"
	"strings"
)

// A Header represents the key-value pairs in an HTTP header.
type Header map[string][]string

// Add adds the key, value pair to the header.
func (h Header) Add(key, value string) {
	h[key] = append(h[key], value)
}

// Set sets the header entries associated with key.
func (h Header) Set(key, value string) {
	h[key] = []string{value}
}

// Get gets the first value associated with the given key.
func (h Header) Get(key string) string {
	return strings.Join(h[key], "")
}

// Write writes a header in wire format.
func (h Header) Write(w io.Writer) error {
	return nil
}
