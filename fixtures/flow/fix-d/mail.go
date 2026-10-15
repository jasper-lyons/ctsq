package fixd

import (
	"bytes"
	"text/template"

	"example.org/ext"
)

var mailTmpl = template.Must(template.New("m").Parse("call {{.Phone}}"))

type Mail struct {
	Phone string
}

func Render(phone string) {
	var buf bytes.Buffer
	mailTmpl.Execute(&buf, Mail{Phone: phone})
	send(buf.String())
}

func RenderWith(t *template.Template, phone string) {
	var buf bytes.Buffer
	t.Execute(&buf, Mail{Phone: phone})
	send(buf.String())
}

func send(s string) {}

func Notify(phone string) {
	var buf bytes.Buffer
	mailTmpl.Execute(&buf, Mail{Phone: phone})
	c := ext.NewClient()
	c.SendEmail(buf.String())
}
