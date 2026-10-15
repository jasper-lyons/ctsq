package main

type Store struct{}

type Runner interface {
	Run()
}

func main() {
	a()
	var s Store
	s.Save()
	run(nil)
}

func a() {
	b()
	d()
}

func b() {
	c()
}

func c() {
}

func d() {
	c()
}

func (s Store) Save() {
	c()
}

func run(r Runner) {
	r.Run()
}

type fast struct{}

func (f fast) Run() {
	c()
}
