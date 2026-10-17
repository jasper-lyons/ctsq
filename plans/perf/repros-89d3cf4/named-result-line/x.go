package a

type Runner interface {
	Run(v string) string
}

type Lazy struct{}

func (l *Lazy) Run(v string) string { return Sink(v) }

func Sink(v string) string { return v }

func Disp(r Runner, s string) string { return r.Run(s) }
