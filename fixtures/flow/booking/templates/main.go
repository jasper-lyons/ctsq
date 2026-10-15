package templates

import (
	"embed"
	"html/template"
	"io"
	"path/filepath"
)

//go:embed *.html.tmpl
var TemplateFiles embed.FS

func Render(w io.Writer, name string, data any) error {
	t, err := template.New(filepath.Base(name)).ParseFS(TemplateFiles, name, "layout.html.tmpl")
	if err != nil {
		return err
	}
	return t.Execute(w, data)
}
