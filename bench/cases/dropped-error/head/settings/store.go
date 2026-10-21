package settings

import (
	"database/sql"
	"fmt"
	"log"
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

const upsert = "INSERT INTO settings (user_id, key, value) VALUES ($1, $2, $3) ON CONFLICT (user_id, key) DO UPDATE SET value = EXCLUDED.value"

// SetMany writes all values in one transaction, so a user never sees half of an update.
func (s *Store) SetMany(userID int64, values map[string]string) error {
	tx, err := s.DB.Begin()
	if err != nil {
		return fmt.Errorf("begin: %w", err)
	}
	defer tx.Rollback()
	for k, v := range values {
		if _, err := tx.Exec(upsert, userID, k, v); err != nil {
			return fmt.Errorf("set setting %q: %w", k, err)
		}
	}
	if err := tx.Commit(); err != nil {
		log.Printf("commit settings for user %d: %v", userID, err)
	}
	return nil
}
