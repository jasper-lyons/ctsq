package main

import (
	"net/http"

	"github.com/go-chi/chi/v5"
)

func listUsers(w http.ResponseWriter, r *http.Request) {}

func createUser(w http.ResponseWriter, r *http.Request) {}

type Users struct{}

func (u *Users) Show(w http.ResponseWriter, r *http.Request) {}

func main() {
	r := chi.NewRouter()
	u := &Users{}
	r.Get("/users", listUsers)
	r.Post("/users", createUser)
	r.Get("/users/{id}", u.Show)
	r.Route("/admin", func(r chi.Router) {
		r.Delete("/users/{id}", createUser)
	})
	http.ListenAndServe(":3000", r)
}
