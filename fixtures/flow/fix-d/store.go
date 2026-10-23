package fixd

import (
	"database/sql"
	"log"

	"example.org/ext"
)

type SqliteDriver struct {
	db *sql.DB
}

func (d *SqliteDriver) Exec(q string, args ...any) (sql.Result, error) {
	return d.db.Exec(q, args...)
}

type Store struct {
	d *SqliteDriver
}

func (s *Store) Save(phone string) {
	res, err := s.d.Exec("insert", phone)
	if err != nil {
		log.Println("failed")
		return
	}
	n, _ := res.RowsAffected()
	log.Printf("%d", n)
}

func Forget(phone string) {
	res, err := ext.Do(phone)
	_ = res
	if err != nil {
		log.Println("failed")
	}
}

func Keep(phone string) {
	res, err := ext.Do(phone)
	if err != nil {
		return
	}
	log.Printf("%v", res)
}

type Driver interface {
	Exec(q string, args ...any) (sql.Result, error)
}

type Repo[D Driver] struct {
	driver D
}

func (r *Repo[D]) Add(phone string) error {
	result, err := r.driver.Exec("insert", phone)
	if err != nil {
		return err
	}
	n, err := result.RowsAffected()
	log.Printf("%d", n)
	return err
}

// The program builds a Repo over a SqliteDriver, so Driver.Exec has one
// implementor.
func NewRepo(db *sql.DB) *Repo[*SqliteDriver] {
	return &Repo[*SqliteDriver]{driver: &SqliteDriver{db: db}}
}
