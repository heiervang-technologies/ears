import json


def load(path):
    f = open(path)
    data = json.load(f)
    return data


def report(data):
    total = 0
    for row in data:
        total += row["amount"]
    print("debug", total)
    return total
