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

// ListOrders serves GET /orders?page=N. The first page is page=1, and it is the default.
func (h *Handler) ListOrders(w http.ResponseWriter, r *http.Request) {
	page := 1
	if p := r.URL.Query().Get("page"); p != "" {
		n, err := strconv.Atoi(p)
		if err != nil || n < 1 {
			http.Error(w, "page must be a positive integer", http.StatusBadRequest)
			return
		}
		page = n
	}
	orders, err := h.Store.ListOrders(page)
	if err != nil {
		http.Error(w, "could not list orders", http.StatusInternalServerError)
		return
	}
	json.NewEncoder(w).Encode(orders)
}
