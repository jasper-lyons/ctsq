package fixd

import (
	"log"
	"reflect"
)

func Fields(row any) {
	v := reflect.ValueOf(row)
	t := v.Type()
	for i := 0; i < t.NumField(); i++ {
		log.Printf("%v", v.Field(i).Interface())
	}
}
