class Inventory:
    def __init__(self):
        self.items = {}

    def add(self, name, count=1):
        self.items[name] = self.items.get(name, 0) + count


inv = Inventory()
inv.add("apple")
