package fixc

import (
	"database/sql"
	"fmt"
	"log"
)

type Booking struct {
	Id    int64
	Phone string
}

type store struct {
	db *sql.DB
}

func (s *store) create(b Booking) (*Booking, bool, error) {
	res, err := s.db.Exec("insert", b.Phone)
	if err != nil {
		return nil, false, err
	}
	_ = res
	return &b, true, nil
}

func (s *store) Handle(b Booking) {
	created, ok, err := s.create(b)
	if err != nil {
		log.Printf("fail %v", err)
	}
	_ = ok
	log.Printf("%v", created)
}

func (s *store) Wrap(b Booking) {
	err := fmt.Errorf("bad %v", b.Phone)
	log.Printf("%v", err)
}

func (s *store) Direct(b Booking) {
	res, err := s.db.Exec("insert", b.Phone)
	if err != nil {
		log.Printf("fail %v", err)
	}
	log.Printf("%v", res)
}
