package fixc

import "log"

type mailer struct{}

func (m *mailer) Send(subj, body string) {
	log.Printf("%s %s", subj, body)
}

type driver struct{}

func (d *driver) Update(stmt string, params []interface{}) {
	log.Printf("%s %v", stmt, params)
}

func (d *driver) Insert(stmt string, xs ...Booking) {
	log.Printf("%s %v", stmt, xs)
}

type App struct {
	mailer *mailer
	driver *driver
}

func ConfirmationEmail(b Booking) string {
	return "Hi " + b.Phone
}

func ToInterfaceSlice(bs []Booking) []interface{} {
	out := []interface{}{}
	for _, b := range bs {
		out = append(out, b)
	}
	return out
}

func ObjectsToUpdateParams(xs []interface{}) []interface{} {
	var out []interface{}
	for _, x := range xs {
		out = append(out, x)
	}
	return out
}

func (a *App) Confirm(b Booking) {
	a.mailer.Send("subj", ConfirmationEmail(b))
}

func (a *App) UpdateAll(bs []Booking) {
	a.driver.Update("update", ObjectsToUpdateParams(ToInterfaceSlice(bs)))
}
