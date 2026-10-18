package main

import "github.com/go-chi/chi/v5"

func digits() {}

// Registered at package level, outside any function, so the fixture adds no
// edge to the call graph pinned in resolve/graph.rs. The rows are what count.
var r = chi.NewRouter()
var _ = r.Get("/q/{id:\\d+}", digits)
