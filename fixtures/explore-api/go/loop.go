package explore

import "fmt"

func Loop(x string, n int) {
	y := x
	for i := 0; i < n; i++ {
		fmt.Println(i, y)
	}
	fmt.Println("done")
}
