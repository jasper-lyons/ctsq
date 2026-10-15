package templates

import (
	"embed"
	"html/template"
	"io"
	"path/filepath"
)

//go:embed *.html.tmpl
var TemplateFiles embed.FS

func Render(w io.Writer, templateString string, ctx any) error {
	t, err := template.New(filepath.Base(templateString)).ParseFS(TemplateFiles, templateString, "layout.html.tmpl")
	if err != nil {
		return err
	}
	return t.Execute(w, ctx)
}
