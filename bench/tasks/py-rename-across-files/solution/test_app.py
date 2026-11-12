import unittest

from app import greeting
from users import get_user


class AppTest(unittest.TestCase):
    def test_greeting(self):
        self.assertEqual(greeting(1), "Hello, Ada")
        self.assertEqual(greeting(9), "Hello, stranger")

    def test_lookup(self):
        self.assertEqual(get_user(2), "Grace")


if __name__ == "__main__":
    unittest.main()
