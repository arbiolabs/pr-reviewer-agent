# inventory

The inventory service tracks stock levels for every warehouse.

## Running

    pip install -e .
    DATABASE_URL=postgresql://localhost/inventory python -m app

## Configuration

The service reads its settings from environment variables when it starts.

| Variable        | Required | Default | Meaning                              |
| --------------- | -------- | ------- | ------------------------------------ |
| `DATABASE_URL`  | yes      |         | The Postgres connection string.      |
| `APP_PORT`      | no       | `8000`  | The port the HTTP server listens on. |
| `APP_LOG_LEVEL` | no       | `info`  | The log level.                       |
