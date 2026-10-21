package api

import (
	"encoding/json"
	"net/http"
	"strconv"

	"example.com/shop/store"
)

type Handler struct{ Store *store.Store }

func (h *Handler) GetOrder(w http.ResponseWriter, r *http.Request) {
	id, err := strconv.ParseInt(r.PathValue("id"), 10, 64)
	if err != nil {
		http.Error(w, "bad order id", http.StatusBadRequest)
		return
	}
	order, err := h.Store.Order(id)
	if err != nil {
		http.Error(w, "order not found", http.StatusNotFound)
		return
	}
	json.NewEncoder(w).Encode(order)
}
