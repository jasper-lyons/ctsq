package fixb

import "fmt"

func Rotate(x string, n int) {
	v0 := x
	var v1 string
	var v2 string
	var v3 string
	var v4 string
	var v5 string
	var v6 string
	var v7 string
	var v8 string
	var v9 string
	var v10 string
	var v11 string
	var v12 string
	var v13 string
	var v14 string
	var v15 string
	var v16 string
	var v17 string
	var v18 string
	var v19 string
	var v20 string
	var v21 string
	var v22 string
	var v23 string
	var v24 string
	var v25 string
	var v26 string
	var v27 string
	var v28 string
	var v29 string
	var v30 string
	var v31 string
	var v32 string
	for i := 0; i < n; i++ {
		v32 = v31
		v31 = v30
		v30 = v29
		v29 = v28
		v28 = v27
		v27 = v26
		v26 = v25
		v25 = v24
		v24 = v23
		v23 = v22
		v22 = v21
		v21 = v20
		v20 = v19
		v19 = v18
		v18 = v17
		v17 = v16
		v16 = v15
		v15 = v14
		v14 = v13
		v13 = v12
		v12 = v11
		v11 = v10
		v10 = v9
		v9 = v8
		v8 = v7
		v7 = v6
		v6 = v5
		v5 = v4
		v4 = v3
		v3 = v2
		v2 = v1
		v1 = v0
	}
	fmt.Println(v32)
}

type Node struct {
	Next *Node
	Val  string
}

func Grow(x string, n int) {
	var b Node
	b.Val = x
	for i := 0; i < n; i++ {
		b.Next = &b
	}
	fmt.Println(b.Val)
}
