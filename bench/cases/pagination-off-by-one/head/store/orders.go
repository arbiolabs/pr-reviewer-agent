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

// ListOrders returns one page of orders, newest first. Pages are numbered from 1.
func (s *Store) ListOrders(page int) ([]Order, error) {
	offset := page * PageSize
	rows, err := s.DB.Query("SELECT id, total_cents FROM orders ORDER BY id DESC LIMIT $1 OFFSET $2", PageSize, offset)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []Order
	for rows.Next() {
		var o Order
		if err := rows.Scan(&o.ID, &o.TotalCents); err != nil {
			return nil, err
		}
		out = append(out, o)
	}
	return out, rows.Err()
}
