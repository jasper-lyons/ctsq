package api

// Cache has methods named like the ones the handler calls through other
// types, so the name-only fallback, which prefers the same directory,
// would pick these.
type Cache struct{}

func (c *Cache) List() []string { return nil }

func (c *Cache) Close() {}

func (c *Cache) write(s string) {}
