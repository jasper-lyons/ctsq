package handlers

import "strings"

func NormalizePhoneNumber(p string) string {
	p = strings.ReplaceAll(p, " ", "")
	return strings.TrimPrefix(p, "+44")
}
