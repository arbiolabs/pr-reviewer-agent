package wallet

import (
	"errors"
	"sync"
)

var ErrInsufficientFunds = errors.New("insufficient funds")

// Wallet is shared by every request handler for one account, so all access to balance goes through mu.
type Wallet struct {
	mu      sync.Mutex
	balance int64
}

func (w *Wallet) Balance() int64 {
	w.mu.Lock()
	defer w.mu.Unlock()
	return w.balance
}

func (w *Wallet) Deposit(cents int64) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.balance += cents
}
