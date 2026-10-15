package fixb

import "fmt"

func BlockShadow(x string) {
	{
		x := "clean"
		_ = x
	}
	fmt.Println(x)
}

func IfInit(x string) {
	if x := "c"; x != "" {
	}
	fmt.Println(x)
}

func InnerDecl(x string, c bool) {
	y := "clean"
	if c {
		y := x
		_ = y
	}
	fmt.Println(y)
}

func PlainAssign(x string, c bool) {
	y := "clean"
	if c {
		y = x
	}
	fmt.Println(y)
}

func RangeShadow(x string, xs []string) {
	for _, x := range xs {
		_ = x
	}
	fmt.Println(x)
}
