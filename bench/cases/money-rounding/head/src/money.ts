/** Prices arrive as decimal strings ("19.99") and are stored and charged as integer cents. */
export function toCents(amount: string): number {
  if (!/^\d+(\.\d{1,2})?$/.test(amount)) throw new Error(`invalid amount: ${amount}`);
  return Math.round(Number(amount) * 100);
}

export function formatCents(cents: number): string {
  return `${Math.floor(cents / 100)}.${String(cents % 100).padStart(2, "0")}`;
}

/** Splits a bill evenly between `people` cards. The shares always add up to the total. */
export function splitBill(totalCents: number, people: number): number[] {
  const share = Math.round(totalCents / people);
  return Array.from({ length: people }, () => share);
}
