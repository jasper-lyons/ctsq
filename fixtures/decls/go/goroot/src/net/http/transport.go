/* A block comment
   before the constraint. */

//go:build !js && (darwin || linux) && go1.21

package http

func FromNotJsFile() {}
