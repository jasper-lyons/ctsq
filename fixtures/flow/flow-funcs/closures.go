package ff

import (
	"fmt"
	"log"
	"net/http"
)

type Handler func(w http.ResponseWriter, r *http.Request)

func Template(name string, data any) Handler {
	return func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprintf(w, "%s %v", name, data)
	}
}

func Direct(token string) {
	func() {
		log.Printf("token %s", token)
	}()
}

func Page(user string) Handler {
	h := Template("page", user)
	return h
}
