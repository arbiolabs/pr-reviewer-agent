import os
from dataclasses import dataclass


@dataclass(frozen=True)
class Config:
    port: int
    log_level: str
    database_url: str


def load() -> Config:
    return Config(
        port=int(os.environ.get("APP_PORT", "8000")),
        log_level=os.environ.get("APP_LOG_LEVEL", "info"),
        database_url=os.environ["DATABASE_URL"],
    )
