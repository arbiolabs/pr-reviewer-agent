import logging
import os

import requests

log = logging.getLogger(__name__)

API_URL = "https://api.payments.example.com/v1"


class PaymentsClient:
    def __init__(self, api_key: str | None = None):
        self.api_key = api_key or os.environ["PAYMENTS_API_KEY"]
        self.session = requests.Session()

    def _headers(self) -> dict[str, str]:
        return {"Authorization": f"Bearer {self.api_key}", "Content-Type": "application/json"}

    def charge(self, customer_id: str, amount_cents: int) -> dict:
        body = {"customer": customer_id, "amount": amount_cents}
        resp = self.session.post(f"{API_URL}/charges", json=body, headers=self._headers(), timeout=10)
        resp.raise_for_status()
        return resp.json()
