class TodoList:
    """A list of things to do, in the order they were added."""

    def __init__(self):
        self.items = []

    def add(self, title):
        self.items.append({"title": title, "done": False})

    def titles(self):
        return [item["title"] for item in self.items]
