import os
from pathlib import Path
import unittest

import pylulang


ROOT = Path(__file__).resolve().parents[2]
LIBRARY = Path(__file__).resolve().parent
COMPILER = os.environ.get("LULANG_BIN", ROOT / "target" / "release" / "lu")


class NumericsTest(unittest.TestCase):
    def test_vector_and_statistics(self):
        with pylulang.compile(LIBRARY / "vector.lu", name="vector", lu=COMPILER) as vector:
            x = [1.0, 2.0, 3.0]
            y = [4.0, 5.0, 6.0]
            self.assertEqual(vector.dot(x, y, 3), 32.0)
            self.assertAlmostEqual(vector.norm2(x, 3), 14.0**0.5)
            vector.axpy(2.0, x, y, 3)
            self.assertEqual(y, [6.0, 9.0, 12.0])
        with pylulang.compile(
            LIBRARY / "statistics.lu", name="statistics", lu=COMPILER
        ) as statistics:
            self.assertEqual(statistics.mean([1.0, 2.0, 3.0], 3), 2.0)
            self.assertAlmostEqual(
                statistics.variance([1.0, 2.0, 3.0], 3), 2.0 / 3.0
            )

    def test_integration_and_linalg(self):
        with pylulang.compile(
            LIBRARY / "integrate.lu", name="integrate", lu=COMPILER
        ) as integrate:
            self.assertEqual(integrate.trapz_uniform([0.0, 1.0, 2.0], 1.0, 3), 2.0)
        with pylulang.compile(LIBRARY / "linalg.lu", name="linalg", lu=COMPILER) as linalg:
            vector = [5.0, 6.0, 0.0, 0.0]
            linalg.gemv_inplace([1.0, 2.0, 3.0, 4.0], vector, 2, 2)
            self.assertEqual(vector[:2], [17.0, 39.0])
            matrix = [1.0, 2.0, 3.0, 4.0]
            linalg.matmul_inplace(matrix, [1.0, 0.0, 0.0, 1.0], [2, 2, 2])
            self.assertEqual(matrix, [1.0, 2.0, 3.0, 4.0])


if __name__ == "__main__":
    unittest.main()
