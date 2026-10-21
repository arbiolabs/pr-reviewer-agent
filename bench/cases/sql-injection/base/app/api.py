import sqlite3

from flask import Flask, jsonify

from app.orders import get_order, orders_for_customer

app = Flask(__name__)


def connect() -> sqlite3.Connection:
    return sqlite3.connect("shop.db")


@app.get("/orders/<int:order_id>")
def show(order_id: int):
    return jsonify(get_order(connect(), order_id))


@app.get("/customers/<customer>/orders")
def by_customer(customer: str):
    return jsonify(orders_for_customer(connect(), customer))
