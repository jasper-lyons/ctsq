package handlers

import (
	"io"
	"strings"

	"example.com/tp/templates"
)

type Item struct {
	Name  string
	Phone string
}

type Page struct {
	Title string
	Items []Item
}

func ShowItems(w io.Writer, it Item) error {
	row := Item{Name: it.Name, Phone: strings.TrimSpace(it.Phone)}
	return templates.Render(w, "page.html.tmpl", Page{Title: "Items", Items: []Item{row}})
}

func ShowAdmin(w io.Writer, items []Item) error {
	return templates.Render(w, "admin.html.tmpl", Page{Title: "Admin", Items: items})
}
