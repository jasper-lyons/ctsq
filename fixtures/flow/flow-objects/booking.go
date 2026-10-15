package fo

import (
	"database/sql"
	"reflect"
	"strings"
)

type Booking struct {
	ID            int
	CustomerPhone string `json:"customer_phone" sql:"customer_phone"`
	Venue         string `json:"venue"`
}

type AdminBooking struct {
	Booking
	Notes string
}

type Repo struct {
	db *sql.DB
}

func (r *Repo) Create(b Booking) error {
	return insert(r.db, "bookings", b)
}

func insert(db *sql.DB, table string, v any) error {
	cols, vals := columns(v)
	_, err := db.Exec("INSERT INTO "+table+" ("+cols+")", vals...)
	return err
}

func columns(v any) (string, []any) {
	rv := reflect.ValueOf(v)
	var names []string
	var vals []any
	for i := 0; i < rv.NumField(); i++ {
		names = append(names, rv.Type().Field(i).Name)
		vals = append(vals, rv.Field(i).Interface())
	}
	return strings.Join(names, ","), vals
}
