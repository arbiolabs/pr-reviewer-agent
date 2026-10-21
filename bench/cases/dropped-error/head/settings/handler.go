package settings

import (
	"encoding/json"
	"net/http"
)

type Handler struct {
	Store  *Store
	UserID func(*http.Request) int64
}

// Put serves PUT /settings with a JSON object of key to value.
func (h *Handler) Put(w http.ResponseWriter, r *http.Request) {
	var values map[string]string
	if err := json.NewDecoder(r.Body).Decode(&values); err != nil {
		http.Error(w, "body must be a JSON object of strings", http.StatusBadRequest)
		return
	}
	if err := h.Store.SetMany(h.UserID(r), values); err != nil {
		http.Error(w, "could not save settings", http.StatusInternalServerError)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}
