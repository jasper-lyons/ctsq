package api

import (
	"net/http"

	"example.com/app/store"
)

type Base struct {
	db *store.Store
}

type Handler struct {
	Base
	store *store.Store
}

// field chain: Store.List, not Cache.List
func (h *Handler) Index() {
	h.store.List()
}

// field promoted from the embedded Base
func (h *Handler) Promoted() {
	h.db.List()
}

// call results: Rows.Close through Store.Query's return type, and
// Store.List through Open's return type
func (h *Handler) Rows() {
	h.store.Query().Close()
	store.Open().List()
}

// a local closure named like Cache.write reaches no definition
func (h *Handler) Footer() {
	write := func(s string) {}
	write("x")
}

// positive control: a same-package method on a typed variable
func (h *Handler) Control() {
	c := &Cache{}
	c.List()
	c.Set("k", "v")
}

// an outside receiver type: net/http.Header.Set, not Cache.Set
func (h *Handler) Header(w http.ResponseWriter) {
	w.Header().Set("Content-Type", "text/plain")
}

// a bare call with nothing of that name in the package or its imports
func (h *Handler) Bare() {
	Open()
}
