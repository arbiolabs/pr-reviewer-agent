import { splitBill, toCents } from "./money.ts";

export type Charge = (card: string, cents: number) => Promise<void>;

/** Charges each card its share of `total`, a decimal string such as "100.00". */
export async function chargeSplit(total: string, cards: string[], charge: Charge): Promise<void> {
  if (cards.length === 0) throw new Error("no cards to charge");
  const shares = splitBill(toCents(total), cards.length);
  for (const [i, card] of cards.entries()) await charge(card, shares[i]!);
}
