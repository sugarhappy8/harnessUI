import os
import sys
import unittest

NONCE = sys.stdin.readline().strip()
sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())

class Check(unittest.TestCase):
    def test_summary(self):
        from report import summary

        self.assertEqual(summary([{"amount": "1.25"}, {"amount": 2}, {}]), "3 orders, total 3.25")
        self.assertEqual(summary([]), "0 orders, total 0.00")

    def test_the_real_file(self):
        from report import load, summary

        self.assertEqual(summary(load("orders.json")), "3 orders, total 20.00")
if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Check)
    result = unittest.TextTestRunner(stream=sys.stderr, verbosity=1).run(suite)
    if result.wasSuccessful():
        print("VERIFIED " + NONCE)
        sys.exit(0)
    sys.exit(1)
