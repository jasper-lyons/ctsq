package fu

import (
	"io"
	"os"
	"strings"
)

const footer = "footer.html.tmpl"

func render(w io.Writer, name string) {
	w.Write([]byte(name))
}

func home(w io.Writer) {
	render(w, "home.html.tmpl")
}

func about(w io.Writer) {
	render(w, "about.html.tmpl")
}

func contact(w io.Writer) {
	render(w, "contact.html.tmpl")
}

func admin(w io.Writer, page string) {
	render(w, "admin-"+page+".html.tmpl")
}

func save(w io.Writer) {
	dir := os.Getenv("DATA_DIR")
	q := strings.TrimSpace(dir)
	w.Write([]byte(q))
}
