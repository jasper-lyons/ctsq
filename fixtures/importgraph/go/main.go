package main

import (
	"fmt"

	"example.com/demo/util"
)

func main() {
	d := util.Distance(1, 2)
	fmt.Println(util.Pretty(d))
}
