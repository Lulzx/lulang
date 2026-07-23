import os
from pathlib import Path
import unittest

import pylulang


SOURCE = """
@c_layout type Vec2 { x: f64, y: f64 }

export fn saxpy(a: f64, x: [f64], y: [f64], n: i64): f64 {
  var total = 0.0
  for i in 0..n {
    y[i] = a * x[i] + y[i]
    total = total + y[i]
  }
  return total
}

export fn positive(x: i64): bool {
  return x > 0
}

export fn borrowed_sum(values: c_slice[f64]): f64 {
  return sum(i in 0..len(values)) values[i]
}

export fn half32(x: f32): f32 {
  return x * f32(0.5)
}

export fn vec2_sum(value: Vec2): f64 {
  return value.x + value.y
}

export fn greeting(prefix: str): str {
  return concat(prefix, "\\0!")
}
"""


class PyLulangTest(unittest.TestCase):
    def test_lists_and_scalars(self):
        root = Path(__file__).resolve().parents[3]
        compiler = os.environ.get("LULANG_BIN", root / "target" / "release" / "lu")
        module = pylulang.compile(SOURCE, name="kernels", lu=compiler)
        x = [1.0, 2.0, 3.0]
        y = [10.0, 20.0, 30.0]
        self.assertEqual(module.saxpy(2.0, x, y, 3), 72.0)
        self.assertEqual(y, [12.0, 24.0, 36.0])
        self.assertTrue(module.positive(1))
        self.assertFalse(module.positive(-1))
        self.assertAlmostEqual(module.half32(9.0), 4.5)
        self.assertEqual(module.vec2_sum((2.5, 4.5)), 7.0)
        self.assertEqual(module.vec2_sum({"x": 3.0, "y": 4.0}), 7.0)
        self.assertEqual(module.greeting("A"), b"A\x00!")
        module.close()

    def test_numpy_without_copying_the_boundary_buffer(self):
        try:
            import numpy
        except ImportError:
            self.skipTest("NumPy is not installed")
        root = Path(__file__).resolve().parents[3]
        compiler = os.environ.get("LULANG_BIN", root / "target" / "release" / "lu")
        with pylulang.compile(SOURCE, name="numpy_kernels", lu=compiler) as module:
            x = numpy.array([1.0, 2.0, 3.0])
            y = numpy.array([10.0, 20.0, 30.0])
            self.assertEqual(module.saxpy(2.0, x, y, 3), 72.0)
            numpy.testing.assert_array_equal(y, [12.0, 24.0, 36.0])
            x.setflags(write=False)
            self.assertEqual(module.borrowed_sum(x), 6.0)


if __name__ == "__main__":
    unittest.main()
