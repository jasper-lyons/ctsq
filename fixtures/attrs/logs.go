package main

import (
	"fmt"
	"log"
	"log/slog"
)

type User struct {
	Email string
}

type Account struct {
	Email string
}

func (u *User) Notify() {
	log.Printf("notify %s", u.Email)
	fmt.Printf("notify %s", u.Email)
	slog.Info("notify", "email", u.Email)
}

func (u *User) CopyTo(a *Account) {
	email := u.Email
	a.Email = email
}
