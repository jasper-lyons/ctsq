package explore

import "fmt"

func Fork(x string, ok bool) {
	y := x
	if ok {
		fmt.Println("yes", y)
	} else {
		fmt.Printf("no %s", y)
	}
}
