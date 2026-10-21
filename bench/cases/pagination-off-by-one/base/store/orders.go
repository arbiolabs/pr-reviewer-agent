package store

import "database/sql"

const PageSize = 20

type Order struct {
	ID         int64
	TotalCents int64
}

type Store struct{ DB *sql.DB }

func (s *Store) Order(id int64) (Order, error) {
	var o Order
	err := s.DB.QueryRow("SELECT id, total_cents FROM orders WHERE id = $1", id).Scan(&o.ID, &o.TotalCents)
	return o, err
}
