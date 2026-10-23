package main

type Alpha struct{}

type Beta struct{}

func (Alpha) Ping() {
	c()
}

func (Beta) Ping() {
	c()
}

func entry() {
	v := lookup()
	v.Ping()
}

// Pinger has two implementors, so v.Ping() is a guess for each.
type Pinger interface{ Ping() }

func lookup() Pinger {
	if true {
		return Alpha{}
	}
	return Beta{}
}
