package pipeline

import (
	"errors"
	"fmt"
)

var ErrEmpty = errors.New("pipeline is empty")

func Load(id string) error {
	if id == "" {
		return ErrEmpty
	}
	return fmt.Errorf("failed to load pipeline %s: %w", id, errors.New("missing"))
}

func Render(n int) error {
	return fmt.Errorf("render %d blocks (%+v)", n, n*2)
}

func Raw() error {
	return errors.New(`raw message`)
}

func RawF(id string) error {
	return fmt.Errorf(`raw %s`, id)
}
