import json


def load_config(path):
    try:
        f = open(path)
        data = json.load(f)
    except FileNotFoundError:
        return {}
    return data


def report(data):
    total = 0
    for row in data:
        total += row["price"]
    return total
