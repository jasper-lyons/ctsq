package handlers

import (
	"log"
	"net/http"

	"example.com/booking/data"
	"example.com/booking/templates"
)

type BookingForm struct {
	Name  string `json:"name"`
	Phone string `json:"phone"`
}

type AdminBooking struct {
	data.Booking
	Notes string
}

type Page struct {
	Title    string
	Bookings []data.Booking
}

type App struct {
	repo   *data.Repository
	mailer Mailer
}

func NewApp(repo *data.Repository, m Mailer) *App {
	return &App{repo: repo, mailer: m}
}

func (a *App) CreateBooking(w http.ResponseWriter, r *http.Request) {
	form, err := ExtractForm[BookingForm](r)
	if err != nil {
		http.Error(w, "bad", 400)
		return
	}
	b := data.Booking{CustomerName: form.Name, CustomerPhone: NormalizePhoneNumber(form.Phone)}
	if err := a.repo.Create(b); err != nil {
		http.Error(w, "fail", 500)
		return
	}
	body := ConfirmationEmail(b)
	a.mailer.Send("customer@example.com", body)
	JSON(w, b)
}

func (a *App) AdminView(w http.ResponseWriter, r *http.Request) {
	b := data.Booking{CustomerName: "Sample", CustomerPhone: "07700900000"}
	ab := AdminBooking{Booking: b, Notes: "walk-in"}
	log.Printf("%v", b)
	JSON(w, ab)
}

func (a *App) Routes() {
	row := data.Booking{CustomerPhone: "07700900001"}
	page := Page{Title: "Bookings", Bookings: []data.Booking{row}}
	http.HandleFunc("/admin/bookings", Template("bookings.html.tmpl", page))
}

func (a *App) ShowDirect(w http.ResponseWriter, r *http.Request) {
	row := data.Booking{CustomerPhone: "07700900002"}
	templates.Render(w, "bookings.html.tmpl", Page{Title: "Direct", Bookings: []data.Booking{row}})
}

func (a *App) Resend(w http.ResponseWriter, r *http.Request) {
	b := data.Booking{CustomerPhone: "07700900003"}
	a.mailer.Send("customer@example.com", ConfirmationEmail(b))
}
