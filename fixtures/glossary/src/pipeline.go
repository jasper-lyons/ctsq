package main

import "errors"

func Pipeline() error {
	return errors.New("cannot start the run")
}

func sync_merged() error {
	return errors.New("nothing was merged")
}

func stockLevel() error {
	return errors.New("the shelf ran dry")
}

func Other() error {
	return errors.New("something else failed")
}
