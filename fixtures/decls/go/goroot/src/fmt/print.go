package fmt

import "io"

func Println(a ...any) (n int, err error) { return 0, nil }

func Fprintf(w io.Writer, format string, a ...any) (n int, err error) { return 0, nil }

type Stringer interface {
	String() string
}
