package http

import (
	"context"
	"io"
	urlpkg "net/url"
)

// A Request represents an HTTP request.
type Request struct {
	Method string
	URL    *urlpkg.URL
	Header Header
	Body   io.ReadCloser
	ctx    context.Context
	Form, PostForm urlpkg.Values
	Trailer        map[string][]string `json:"trailer"`
}

// NewRequest wraps NewRequestWithContext.
func NewRequest(method, target string, body io.Reader) (*Request, error) {
	return NewRequestWithContext(context.Background(), method, target, body)
}

// NewRequestWithContext returns a new Request.
func NewRequestWithContext(ctx context.Context, method, target string, body io.Reader) (*Request, error) {
	return nil, nil
}

// Context returns the request's context.
func (r *Request) Context() context.Context {
	return r.ctx
}

// Cookies parses and returns the cookies.
func (r *Request) Cookies(names ...string) []*Cookie {
	return nil
}

type Cookie struct {
	Name  string
	Value string
}

// Set is generic, to check type parameters.
type Set[T comparable] struct {
	items map[T]struct{}
}

func (s *Set[T]) Add(v T) bool { return true }

func Keys[K comparable, V any](m map[K]V) []K { return nil }

const (
	MethodGet  = "GET"
	MethodPost = "POST"
)

type ConnState int

const (
	StateNew ConnState = iota
	StateActive
	StateIdle
)

var ErrMissingFile = errors.New("http: no such file")
