package fixc

func (a *App) Create(phone string, ids []int64) {
	var bookings []Booking
	bookings = append(bookings, Booking{Phone: phone})
	for i := range bookings {
		if ids[i] > 0 {
			bookings[i].Id = ids[i]
		}
	}
	a.driver.Insert("insert", bookings...)
}

func (a *App) CreateIndexed(phone string) {
	var bookings []Booking
	bookings = append(bookings, Booking{Phone: phone})
	for i := range bookings {
		bookings[i].Id = int64(i)
	}
	a.driver.Insert("insert", bookings...)
}
