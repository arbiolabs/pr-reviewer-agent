## Automated PR review

Reviewed commit: `f86859b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7`

Found 2 actionable issues.

- High: Payment captured before the order is confirmed (`src/checkout/capture.ts:42`)
- Medium (unconfirmed): Retry loop has no upper bound (`src/checkout/retry.ts:17`)

Still present from an earlier commit:

- Low: Log line prints the card's last four digits (`src/checkout/log.ts:9`, first reported on `b131fd0`)

Review performed with isolated read-only tools. Human review is still required.
