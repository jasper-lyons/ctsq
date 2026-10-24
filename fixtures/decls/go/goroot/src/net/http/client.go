package http

import (
	"io"
	"time"
)

// conn is embedded in Client to check promotion.
type conn struct {
	idle time.Duration
}

func (c *conn) Close() error { return nil }

// A Client is an HTTP client.
type Client struct {
	*conn
	io.Reader
	Timeout time.Duration
}

// Do sends an HTTP request and returns an HTTP response.
func (c *Client) Do(req *Request) (*Response, error) {
	return nil, nil
}

// broken has a body that does not parse, but its braces balance.
func broken() {
	x := := {
		if ) ( {
		}
	}
}

// DefaultClient is the default Client.
var DefaultClient = &Client{}

var zeroClient Client

var defaultTimeout = newTimeout()

func newTimeout() time.Duration { return 0 }

type Response struct {
	Status string
}

// ReadCloser embeds an interface from the same package.
type ReadCloser interface {
	Closer
	Read(p []byte) (n int, err error)
}

type Closer interface {
	Close() error
}

// AfterBroken comes after the broken function.
func AfterBroken() int { return 1 }
