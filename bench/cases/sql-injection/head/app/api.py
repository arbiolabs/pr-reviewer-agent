import sqlite3

from flask import Flask, jsonify, request

from app.orders import get_order, orders_for_customer, search_orders

app = Flask(__name__)


def connect() -> sqlite3.Connection:
    return sqlite3.connect("shop.db")


@app.get("/orders/<int:order_id>")
def show(order_id: int):
    return jsonify(get_order(connect(), order_id))


@app.get("/customers/<customer>/orders")
def by_customer(customer: str):
    return jsonify(orders_for_customer(connect(), customer))


@app.get("/orders/search")
def search():
    return jsonify(search_orders(connect(), request.args.get("q", ""), request.args.get("sort", "id")))
