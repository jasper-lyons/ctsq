package fo

import (
	"encoding/json"
	"net/http"
)

type Form struct {
	Phone string `json:"phone"`
	Name  string `json:"name"`
}

func ExtractForm[T any](r *http.Request) (T, error) {
	var v T
	err := json.NewDecoder(r.Body).Decode(&v)
	return v, err
}

func JSON[T any](w http.ResponseWriter, v T) {
	json.NewEncoder(w).Encode(v)
}
