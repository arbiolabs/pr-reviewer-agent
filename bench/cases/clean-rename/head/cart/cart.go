package cart

type Item struct {
	PriceCents int64
	Quantity   int64
}

// subtotal returns the sum of price times quantity over items.
func subtotal(items []Item) int64 {
	var total int64
	for _, it := range items {
		total += it.PriceCents * it.Quantity
	}
	return total
}
