import os
import sys
import unittest

NONCE = sys.stdin.readline().strip()
sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())

class Check(unittest.TestCase):
    def test_pending_and_complete(self):
        from todo import TodoList

        todos = TodoList()
        for title in ["a", "b", "c"]:
            todos.add(title)
        self.assertEqual(todos.pending(), ["a", "b", "c"])
        todos.complete("b")
        self.assertEqual(todos.pending(), ["a", "c"])
        self.assertEqual(todos.titles(), ["a", "b", "c"])
        todos.complete("a")
        todos.complete("c")
        self.assertEqual(todos.pending(), [])

    def test_unknown_title(self):
        from todo import TodoList

        todos = TodoList()
        todos.add("a")
        with self.assertRaises(KeyError):
            todos.complete("missing")
if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Check)
    result = unittest.TextTestRunner(stream=sys.stderr, verbosity=1).run(suite)
    if result.wasSuccessful():
        print("VERIFIED " + NONCE)
        sys.exit(0)
    sys.exit(1)
