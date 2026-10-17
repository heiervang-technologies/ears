class Inventory:
    def __init__(self):
        self.items = {}

    def add(self, name, count=1):
        self.items[name] = self.items.get(name, 0) + count

    def remove(self, name):
        if name not in self.items:
            raise KeyError(name)
        del self.items[name]

    def total(self):
        return sum(self.items.values())


inv = Inventory()
inv.add("apple")
