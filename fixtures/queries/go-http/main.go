package main

import (
	"net/http"
)

type Handler struct{}

func (h *Handler) healthz(w http.ResponseWriter, r *http.Request) {}

func (h *Handler) login(w http.ResponseWriter, r *http.Request) {}

func (h *Handler) logout(w http.ResponseWriter, r *http.Request) {}

func (h *Handler) authed(next http.HandlerFunc) http.HandlerFunc { return next }

func index(w http.ResponseWriter, r *http.Request) {}

func (h *Handler) Routes(mux *http.ServeMux) {
	mux.HandleFunc("GET /api/healthz", h.healthz)
	mux.HandleFunc("POST /api/auth/login", h.login)
	mux.HandleFunc("POST /api/auth/logout", h.authed(h.logout))
	mux.Handle("/static/", http.NotFoundHandler())
}

func main() {
	h := &Handler{}
	mux := http.NewServeMux()
	h.Routes(mux)
	http.HandleFunc("/", index)
	http.ListenAndServe(":8080", mux)
}
