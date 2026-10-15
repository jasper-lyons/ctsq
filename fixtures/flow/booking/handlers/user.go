package handlers

import (
	"fmt"
	"io"
	"strings"

	"example.com/booking/templates"
)

type User struct {
	Username string
}

func handle(w io.Writer, name string) {
	fmt.Printf("hello %s", name)
}

func ShowUser(w io.Writer, u User) error {
	return templates.Render(w, "user.html.tmpl", User{Username: strings.ToLower(u.Username)})
}
