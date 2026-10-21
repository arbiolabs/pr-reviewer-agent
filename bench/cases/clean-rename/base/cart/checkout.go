package cart

// Total is what the customer pays: the items plus shipping.
func Total(items []Item, shippingCents int64) int64 {
	return calc(items) + shippingCents
}

// FreeShipping reports whether the items alone reach the free-shipping threshold.
func FreeShipping(items []Item, thresholdCents int64) bool {
	return calc(items) >= thresholdCents
}
