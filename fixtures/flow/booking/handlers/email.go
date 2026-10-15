package handlers

import (
	"bytes"
	"html/template"

	"example.com/booking/data"
)

type Mailer interface {
	Send(to string, body string) error
}

type FakeMailer struct{}

func (FakeMailer) Send(to string, body string) error { return nil }

const confirmText = `Hello {{ .CustomerName }},
we will call you on {{ .CustomerPhone }}.`

var confirmTmpl = template.Must(template.New("confirm").Parse(confirmText))

func ConfirmationEmail(b data.Booking) string {
	var buf bytes.Buffer
	confirmTmpl.Execute(&buf, b)
	return buf.String()
}

func NewMailer() Mailer {
	return FakeMailer{}
}
