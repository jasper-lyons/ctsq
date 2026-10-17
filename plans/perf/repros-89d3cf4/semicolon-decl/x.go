package a

func Sink(v string) string { return v }

func Entry(s string) string { return OneLine(s) }

type R interface{ Run(v string) string }

func Disp(r R, s string) string { return r.Run(s) }
