import logging
import os

import requests

log = logging.getLogger(__name__)

API_URL = "https://api.payments.example.com/v1"
ATTEMPTS = 3


class PaymentsClient:
    def __init__(self, api_key: str | None = None):
        self.api_key = api_key or os.environ["PAYMENTS_API_KEY"]
        self.session = requests.Session()

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self.api_key}", "Content-Type": "application/json"}

    def charge(self, customer_id: str, amount_cents: int, idempotency_key: str) -> dict:
        body = {"customer": customer_id, "amount": amount_cents}
        headers = {**self._headers(), "Idempotency-Key": idempotency_key}
        for attempt in range(1, ATTEMPTS + 1):
            resp = self.session.post(f"{API_URL}/charges", json=body, headers=headers, timeout=10)
            if resp.status_code < 500:
                break
            log.warning("charge attempt %d/%d failed: status=%s headers=%s body=%s", attempt, ATTEMPTS, resp.status_code, headers, body)
        resp.raise_for_status()
        return resp.json()
