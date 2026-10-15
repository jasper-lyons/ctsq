package fc

import (
	"encoding/json"
	"io"
	"log"
	"net/http"
)

type Booking struct {
	CustomerPhone string
	Venue         string
}

func handle(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(r.Body)
	out := first(string(body))
	log.Printf("got %s", out)
	data, _ := json.Marshal(out)
	w.Write(data)
}

func first(s string) string {
	t := second(s)
	return t
}

func second(s string) string {
	return third(s)
}

func third(s string) string {
	n := normalise(s)
	return n
}

func remind(b Booking) {
	phone := b.CustomerPhone
	log.Printf("text %s", phone)
}

func archive(b Booking) {
	data, _ := json.Marshal(b.CustomerPhone)
	_ = data
}
