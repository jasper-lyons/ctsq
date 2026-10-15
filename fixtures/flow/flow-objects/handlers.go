package fo

import (
	"fmt"
	"log"
	"net/http"
	"strings"
)

var cache = map[string]Booking{}

var repo *Repo

func normalise(s string) string {
	return strings.TrimSpace(s)
}

func createBooking(w http.ResponseWriter, r *http.Request) {
	form, err := ExtractForm[Form](r)
	if err != nil {
		return
	}
	phone := normalise(form.Phone)
	b := Booking{CustomerPhone: phone}
	repo.Create(b)
	log.Printf("created %v", b)
	cache[phone] = b
	JSON[Booking](w, b)
}

func lookup(phone string) Booking {
	return cache[phone]
}

func adminView(w http.ResponseWriter, ab AdminBooking) {
	phone := ab.CustomerPhone
	fmt.Fprintf(w, "%s", phone)
	log.Printf("admin %s", phone)
}
