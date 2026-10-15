package fixb

import (
	"fmt"

	"github.com/other/lib"
)

type Booking struct {
	Phone string
	Name  string
}

func Through(x string) {
	u := lib.Do(x)
	fmt.Println(u)
}

func Copy(x []byte) {
	dst := make([]byte, 4)
	copy(dst, x)
	fmt.Println(dst)
}

func Append(x string) {
	var out []string
	out = append(out, x)
	fmt.Println(out)
}

func Overwrite(x string) {
	var b Booking
	b.Phone = x
	b.Phone = "clean"
	fmt.Println(b.Phone)
}

func OtherField(x string) {
	var b Booking
	b.Phone = x
	b.Name = "n"
	fmt.Println(b.Phone)
}
