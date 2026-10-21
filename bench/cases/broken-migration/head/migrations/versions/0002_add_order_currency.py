"""add currency to orders"""

import sqlalchemy as sa
from alembic import op

revision = "0002"
down_revision = "0001"


def upgrade() -> None:
    op.add_column("orders", sa.Column("currency", sa.String(3), nullable=False))


def downgrade() -> None:
    op.drop_column("orders", "currency")
