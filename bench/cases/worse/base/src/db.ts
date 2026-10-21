export type Order = { id: string; userId: string; totalCents: number; refundedCents: number; status: "paid" | "refunded" | "cancelled" };
export type Profile = { id: string; name: string; email: string };

export const db = {
  orders: new Map<string, Order>(),
  profiles: new Map<string, Profile>(),
};

/** Queues a refund of `cents` with the payment provider, which retries until the provider accepts it. */
export declare function queueRefund(orderId: string, cents: number): void;
