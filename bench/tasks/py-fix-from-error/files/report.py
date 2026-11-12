import json


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def summary(orders):
    total = sum(order["amount"] for order in orders)
    return f"{len(orders)} orders, total {total:.2f}"


if __name__ == "__main__":
    print(summary(load("orders.json")))
