import os
from pathlib import Path
import unittest

import pylulang


SOURCE = """
@c_layout type Vec2 { x: f64, y: f64 }
@c_layout type LuResultI64 { status: i64, value: i64 }

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

export fn bump(values: c_mut_slice[f64]): f64 {
  for i in 0..len(values) {
    values[i] = values[i] + 1.0
  }
  return values[0]
}

export fn half32(x: f32): f32 {
  return x * f32(0.5)
}

export fn vec2_sum(value: Vec2): f64 {
  return value.x + value.y
}

export fn make_vec2(x: f64, y: f64): Vec2 {
  return Vec2 { x, y }
}

export fn make_values(count: i64): [f64] {
  var values = arr(count, 0.0)
  for i in 0..count {
    values[i] = float(i) * 0.5
  }
  return values
}

export fn checked_div(numerator: i64, denominator: i64): LuResultI64 {
  if denominator == 0 {
    return LuResultI64 { 1, 0 }
  }
  return LuResultI64 { 0, numerator / denominator }
}

export fn callback_identity(
  callback: c_fn[(i64) -> i64],
): c_fn[(i64) -> i64] {
  return callback
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
        made = module.make_vec2(1.25, 3.75)
        self.assertEqual((made.x, made.y), (1.25, 3.75))
        with module.make_values(4) as owned:
            self.assertEqual(list(owned), [0.0, 0.5, 1.0, 1.5])
            owned[0] = 9.0
            self.assertEqual(owned[0], 9.0)
        self.assertEqual(module.checked_div(12, 3), 4)
        with self.assertRaisesRegex(pylulang.LulangError, "status 1"):
            module.checked_div(12, 0)
        callback = module.callback_identity(lambda value: value + 1)
        self.assertEqual(callback(41), 42)
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
            self.assertEqual(module.bump(y), 13.0)
            numpy.testing.assert_array_equal(y, [13.0, 25.0, 37.0])
            with self.assertRaises(TypeError):
                module.bump(x)


if __name__ == "__main__":
    unittest.main()
