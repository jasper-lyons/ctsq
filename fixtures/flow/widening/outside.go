package wd

import (
	"bytes"
	"fmt"
	"strings"
)

// Sink has one implementation in the module and two from the standard library.
type Sink interface {
	Write(p []byte) (int, error)
}

type fileSink struct{}

func (f *fileSink) Write(p []byte) (int, error) {
	fmt.Println(string(p))
	return len(p), nil
}

func emit(s Sink, data []byte) {
	s.Write(data)
}

func wire() {
	emit(&fileSink{}, nil)
	emit(&bytes.Buffer{}, nil)
	emit(&strings.Builder{}, nil)
}

// Notifier has an implementation that nothing constructs.
type Notifier interface {
	Notify(msg string)
}

type quietNotifier struct{}

func (q *quietNotifier) Notify(msg string) {
	fmt.Println(msg)
}

func tell(n Notifier, x string) {
	n.Notify(x)
}
