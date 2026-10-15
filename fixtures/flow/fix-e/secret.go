package fe

import (
	"fmt"

	"example.com/other/y"
	"strings"
)

func compare(secret string) {
	ok := secret == "a"
	fmt.Println(ok)
	less := secret < "m"
	fmt.Println(less)
	fmt.Println(secret + "!")
}

func chain(secret string) {
	up := strings.ToUpper(secret)
	fmt.Println(up)
}

func unknown(secret string) {
	v := y.Do(secret)
	fmt.Println(v)
}
