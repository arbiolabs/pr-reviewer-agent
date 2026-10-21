import sqlite3


def get_order(conn: sqlite3.Connection, order_id: int):
    return conn.execute("SELECT id, customer, total_cents FROM orders WHERE id = ?", (order_id,)).fetchone()


def orders_for_customer(conn: sqlite3.Connection, customer: str):
    return conn.execute("SELECT id, total_cents FROM orders WHERE customer = ? ORDER BY id", (customer,)).fetchall()


def search_orders(conn: sqlite3.Connection, term: str, sort: str = "id"):
    query = f"SELECT id, customer, total_cents FROM orders WHERE customer LIKE '%{term}%' ORDER BY {sort}"
    return conn.execute(query).fetchall()
