module example.com/project

go 1.25

require example.com/lib v1.2.3

require (
	github.com/BigCorp/thing v0.4.0 // indirect
	example.com/local v0.0.0
	example.com/old v0.9.0
	example.com/missing v1.0.0
)

replace example.com/local => ./local

replace (
	example.com/old v0.9.0 => example.com/lib v1.2.3 // swapped
)
