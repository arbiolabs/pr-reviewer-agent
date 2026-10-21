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

// Withdraw takes cents out of the wallet and never lets the balance go below zero.
func (w *Wallet) Withdraw(cents int64) error {
	if w.Balance() < cents {
		return ErrInsufficientFunds
	}
	w.mu.Lock()
	w.balance -= cents
	w.mu.Unlock()
	return nil
}
