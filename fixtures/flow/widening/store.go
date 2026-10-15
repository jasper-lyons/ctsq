package wd

import (
	"fmt"
	"log"
)

// Store has three implementations. Two are constructed, ghostStore is not.
type Store interface {
	Save(v string) error
}

type dbStore struct{}

func (d *dbStore) Save(v string) error {
	fmt.Println("db", v)
	return nil
}

type MemStore struct{}

func NewMemStore() *MemStore {
	return &MemStore{}
}

func (m *MemStore) Save(v string) error {
	log.Println("mem", v)
	return nil
}

type ghostStore struct{}

func (g *ghostStore) Save(v string) error {
	fmt.Println("ghost", v)
	return nil
}
