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
