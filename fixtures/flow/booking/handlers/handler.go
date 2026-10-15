package handlers

import (
	"encoding/json"
	"net/http"

	"example.com/booking/templates"
)

func JSON[T any](w http.ResponseWriter, v T) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(v)
}

func ExtractForm[T any](r *http.Request) (T, error) {
	var form T
	err := json.NewDecoder(r.Body).Decode(&form)
	return form, err
}

// Template returns a handler closure that renders a named page.
func Template(name string, data any) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		templates.Render(w, name, data)
	}
}
