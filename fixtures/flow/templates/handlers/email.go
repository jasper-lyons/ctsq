package handlers

import (
	"html/template"
	"io"
)

const emailText = `Hello {{ .Name }},
we will call you on {{ .Phone }}.
Thanks`

var emailTmpl = template.Must(template.New("email").Parse(emailText))

func Confirm(w io.Writer, it Item) error {
	phone := it.Phone
	return emailTmpl.Execute(w, Item{Name: it.Name, Phone: phone})
}
