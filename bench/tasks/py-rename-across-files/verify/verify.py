import os
import sys
import unittest

NONCE = sys.stdin.readline().strip()
sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())

class Check(unittest.TestCase):
    def test_renamed(self):
        import users
        from app import greeting

        self.assertEqual(users.get_user(1), "Ada")
        self.assertFalse(hasattr(users, "get_usr"))
        self.assertEqual(greeting(2), "Hello, Grace")
        self.assertEqual(greeting(5), "Hello, stranger")
if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Check)
    result = unittest.TextTestRunner(stream=sys.stderr, verbosity=1).run(suite)
    if result.wasSuccessful():
        print("VERIFIED " + NONCE)
        sys.exit(0)
    sys.exit(1)
