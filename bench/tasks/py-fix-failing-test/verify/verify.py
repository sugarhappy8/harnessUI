import os
import sys
import unittest

NONCE = sys.stdin.readline().strip()
sys.dont_write_bytecode = True
sys.path.insert(0, os.getcwd())

class Check(unittest.TestCase):
    def test_mean(self):
        from stats import mean

        self.assertEqual(mean([2, 4, 6]), 4)
        self.assertEqual(mean([5]), 5)
        self.assertEqual(mean([1, 2]), 1.5)

    def test_median_still_works(self):
        from stats import median

        self.assertEqual(median([4, 1, 3, 2]), 2.5)
        self.assertEqual(median([7]), 7)
if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(Check)
    result = unittest.TextTestRunner(stream=sys.stderr, verbosity=1).run(suite)
    if result.wasSuccessful():
        print("VERIFIED " + NONCE)
        sys.exit(0)
    sys.exit(1)
