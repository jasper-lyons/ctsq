package main

import "fmt"

type Server struct {
	host string
	port int
}

func (s *Server) Start() {
	fmt.Println("starting")
}

func NewServer(host string, port int) *Server {
	return &Server{host: host, port: port}
}

func main() {
	s := NewServer("localhost", 8080)
	s.Start()
}
