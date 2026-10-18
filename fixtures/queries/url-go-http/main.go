package main

import "net/http"

func home(w http.ResponseWriter, r *http.Request)  {}
func about(w http.ResponseWriter, r *http.Request) {}
func getEv(w http.ResponseWriter, r *http.Request) {}
func postEv(w http.ResponseWriter, r *http.Request) {}
func digit(w http.ResponseWriter, r *http.Request) {}
func dup(w http.ResponseWriter, r *http.Request)   {}

// Registered at package level, outside any function, so the fixture adds no
// edge to the call graph pinned in resolve/graph.rs. The rows are what count.
var mux = http.NewServeMux()
var _ = mux.HandleFunc("/{$}", home)
var _ = mux.HandleFunc("/about/{$}", about)
var _ = mux.HandleFunc("GET /events/{id}", getEv)
var _ = mux.HandleFunc("POST /events/{id}", postEv)
var _ = mux.HandleFunc("GET /x/{1}", digit)
var _ = mux.HandleFunc("GET /a/{id}/b/{id}", dup)
