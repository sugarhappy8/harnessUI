import unittest

from todo import TodoList


class TodoTest(unittest.TestCase):
    def test_add(self):
        todos = TodoList()
        todos.add("write")
        todos.add("test")
        self.assertEqual(todos.titles(), ["write", "test"])


if __name__ == "__main__":
    unittest.main()
