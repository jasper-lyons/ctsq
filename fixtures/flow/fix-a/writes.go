package fixa

import "fmt"

type Contact struct {
	Phone string
}

type Booking struct {
	Phone   string
	Contact Contact
}

func fill(b *Booking, s string) {
	b.Phone = s
}

func (b *Booking) SetPhone(p string) {
	b.Phone = p
}

func ParamWrite(x string) {
	var b Booking
	fill(&b, x)
	fmt.Println(b.Phone)
}

func ReceiverWrite(x string) {
	b := &Booking{}
	b.SetPhone(x)
	fmt.Println(b.Phone)
}

func AliasWrite(x string) {
	b := &Booking{}
	c := b
	c.Phone = x
	fmt.Println(b.Phone)
}

func DerefWrite(x string) {
	var y string
	p := &y
	*p = x
	fmt.Println(y)
}

func CaptureWrite(x string) {
	var out string
	func() {
		out = x
	}()
	fmt.Println(out)
}

func two(s string) (string, error) {
	return s, nil
}

func named(s string) (v string, err error) {
	v = s
	return
}

func SecondResult(x string) {
	_, err := two(x)
	fmt.Println(err)
}

func FirstResult(x string) {
	v, _ := two(x)
	fmt.Println(v)
}

func NamedResults(x string) {
	v, err := named(x)
	fmt.Println(err)
	fmt.Println(v)
}

func Nested(b Booking) {
	fmt.Println(b.Contact.Phone)
	fmt.Println(b.Phone)
}

func slot() *string {
	return new(string)
}

func UnknownPointer(x string) {
	p := slot()
	*p = x
	fmt.Println(*p)
}
