package wd

func main() {
	run(&dbStore{}, "a")
	run(NewMemStore(), "b")
	narrow(NewMemStore(), "c")
	pick(&dbStore{}, "d")
}

func run(s Store, x string) {
	s.Save(x)
}

func narrow(s Store, x string) {
	if m, ok := s.(*MemStore); ok {
		m.Save(x)
	}
}

func pick(s Store, x string) {
	switch t := s.(type) {
	case *dbStore:
		t.Save(x)
	default:
		s.Save(x)
	}
}
