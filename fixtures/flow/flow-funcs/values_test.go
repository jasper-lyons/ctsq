package ff

import "testing"

func TestSignup(t *testing.T) {
	sendConfirmation = func(to string) {}
	Signup("a@b.c")
}
