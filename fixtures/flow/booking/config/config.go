package config

import "errors"

type Config struct {
	Addr string
}

func parseConfig(raw string) (Config, error) {
	c := Config{Addr: raw}
	if err := validate(c); err != nil {
		return c, err
	}
	return c, nil
}

func validate(c Config) error {
	if c.Addr == "" {
		return errors.New("no address")
	}
	return nil
}
