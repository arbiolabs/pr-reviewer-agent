package settings

import (
	"database/sql"
	"fmt"
)

type Store struct{ DB *sql.DB }

func (s *Store) Get(userID int64, key string) (string, error) {
	var v string
	err := s.DB.QueryRow("SELECT value FROM settings WHERE user_id = $1 AND key = $2", userID, key).Scan(&v)
	if err != nil {
		return "", fmt.Errorf("get setting %q: %w", key, err)
	}
	return v, nil
}
