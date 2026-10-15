package ff

import "log"

// One assignment in the module: calls resolve to sendEmail.
var sendConfirmation = sendEmail

// Two assignments in the module: calls stop.
var notify = sendEmail

func sendEmail(to string) {
	log.Printf("mail %s", to)
}

func sendSMS(to string) {
	log.Printf("sms %s", to)
}

func helper(s string) {
	log.Printf("helper %s", s)
}

func useSMS() {
	notify = sendSMS
}

func Signup(email string) {
	sendConfirmation(email)
}

func Local(name string) {
	f := helper
	f(name)
}

func Alert(phone string) {
	notify(phone)
}

type Notifier interface {
	Notify(msg string)
}

func Broadcast(n Notifier, msg string) {
	n.Notify(msg)
}

func Twice(name string, loud bool) {
	g := helper
	if loud {
		g = sendSMS
	}
	g(name)
}
