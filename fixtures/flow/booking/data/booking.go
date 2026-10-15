package data

import (
	"database/sql"
	"reflect"
	"strings"
)

type Booking struct {
	ID            int    `json:"id" sql:"id"`
	CustomerName  string `json:"customer_name" sql:"customer_name"`
	CustomerPhone string `json:"customer_phone" sql:"customer_phone"`
}

type StorageDriver interface {
	Insert(table string, row any) error
}

type SQLDriver struct {
	db *sql.DB
}

func NewSQLDriver(db *sql.DB) *SQLDriver {
	return &SQLDriver{db: db}
}

func (d *SQLDriver) Insert(table string, row any) error {
	return insertRow(d.db, table, row)
}

// insertRow builds the statement from sql tags with reflection.
func insertRow(db *sql.DB, table string, row any) error {
	v := reflect.ValueOf(row)
	t := v.Type()
	var cols []string
	var args []any
	for i := 0; i < t.NumField(); i++ {
		cols = append(cols, t.Field(i).Tag.Get("sql"))
		args = append(args, v.Field(i).Interface())
	}
	q := "INSERT INTO " + table + " (" + strings.Join(cols, ",") + ") VALUES (?,?,?)"
	_, err := db.Exec(q, args...)
	return err
}

type Repository struct {
	driver StorageDriver
}

func NewRepository(d StorageDriver) *Repository {
	return &Repository{driver: d}
}

func (r *Repository) Create(b Booking) error {
	return r.driver.Insert("bookings", b)
}
