package store

type Store struct{}

type Rows struct{}

func Open() *Store { return &Store{} }

func (s *Store) List() []string { return nil }

func (s *Store) Query() *Rows { return &Rows{} }

func (r *Rows) Close() {}
