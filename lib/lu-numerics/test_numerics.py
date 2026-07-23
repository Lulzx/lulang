import os
from pathlib import Path
import shutil
import subprocess
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

    def test_complete_package_and_comparison_coverage(self):
        with pylulang.compile(
            LIBRARY / "src/lib.lu", name="lu_numerics", lu=COMPILER
        ) as numerics:
            x = [1.0, 2.0, 3.0, 4.0]
            y = [4.0, 3.0, 2.0, 1.0]
            self.assertEqual(numerics.dot(x, y, 4), 20.0)
            self.assertAlmostEqual(numerics.norm2(x, 4), 30.0**0.5)
            numerics.axpy(2.0, x, y, 4)
            self.assertEqual(y, [6.0, 7.0, 8.0, 9.0])
            numerics.scale(0.5, y, 4)
            self.assertEqual(y, [3.0, 3.5, 4.0, 4.5])
            self.assertEqual(numerics.mean(x, 4), 2.5)
            self.assertEqual(numerics.variance(x, 4), 1.25)
            self.assertEqual(numerics.mse(x, [1.0, 2.0, 3.0, 4.0], 4), 0.0)
            self.assertEqual(numerics.trapz_uniform(x, 1.0, 4), 7.5)
            self.assertEqual(
                numerics.trapz_xy([0.0, 1.0, 2.0, 3.0], x, 4), 7.5
            )
            self.assertEqual(numerics.polynomial_eval([1.0, 2.0, 3.0], 2.0, 3), 17.0)
            vector = [5.0, 6.0, 0.0, 0.0]
            numerics.gemv_inplace([1.0, 2.0, 3.0, 4.0], vector, 2, 2)
            self.assertEqual(vector[:2], [17.0, 39.0])
            matrix = [1.0, 2.0, 3.0, 4.0]
            numerics.matmul_square_inplace(
                matrix, [1.0, 0.0, 0.0, 1.0], 2
            )
            self.assertEqual(matrix, [1.0, 2.0, 3.0, 4.0])
            signal = [1.0, 2.0, 3.0, 4.0]
            numerics.convolution_inplace(signal, [1.0, 0.5], 4, 2)
            self.assertEqual(signal, [1.0, 2.5, 4.0, 5.5])
            numerics.moving_average_inplace(signal, 2, 4)
            self.assertEqual(signal, [1.0, 1.75, 3.25, 4.75])
            self.assertEqual(numerics.lcg_step(7), numerics.lcg_step(7))
            self.assertTrue(2.9 < numerics.monte_carlo_pi(20000, 7) < 3.4)
            self.assertAlmostEqual(numerics.bisect_sqrt(2.0, 50), 2.0**0.5)
            self.assertEqual(numerics.sinc(0.0), 1.0)
            self.assertAlmostEqual(numerics.normal_pdf(0.0), 0.3989422804014327)
            self.assertEqual(numerics.sigmoid(0.0), 0.5)
            self.assertEqual(numerics.clamp(3.0, 0.0, 2.0), 2.0)
            self.assertEqual(numerics.lerp(2.0, 6.0, 0.25), 3.0)
            self.assertEqual(
                numerics.distance3(0.0, 0.0, 0.0, 1.0, 2.0, 2.0), 3.0
            )
            self.assertEqual(numerics.determinant2(1.0, 2.0, 3.0, 4.0), -2.0)
            self.assertEqual(numerics.factorial(10), 3628800)
            self.assertEqual(numerics.binomial(10, 4), 210)

            exports = {item["name"] for item in numerics.manifest["exports"]}
            rows = [
                line.split("\t")
                for line in (LIBRARY / "comparisons/functions.tsv")
                .read_text()
                .splitlines()[1:]
                if line
            ]
            self.assertEqual({row[0] for row in rows}, exports)
            for row in rows:
                name = row[0]
                for source_name in row[2:5]:
                    source = (LIBRARY / "comparisons" / source_name).read_text()
                    self.assertIn(f"{name}(", source)

        compiler = shutil.which("clang++")
        if compiler:
            subprocess.run(
                [
                    compiler,
                    "-std=c++17",
                    "-fsyntax-only",
                    str(LIBRARY / "comparisons/numerics.cpp"),
                ],
                check=True,
            )


if __name__ == "__main__":
    unittest.main()
