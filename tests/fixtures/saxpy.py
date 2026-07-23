import ctypes
import sys

library = ctypes.CDLL(sys.argv[1])
double_pointer = ctypes.POINTER(ctypes.c_double)
library.saxpy.argtypes = [
    ctypes.c_double,
    double_pointer,
    ctypes.c_int64,
    double_pointer,
    ctypes.c_int64,
    ctypes.c_int64,
]
library.saxpy.restype = ctypes.c_double
x = (ctypes.c_double * 3)(1.0, 2.0, 3.0)
y = (ctypes.c_double * 3)(10.0, 20.0, 30.0)
total = library.saxpy(2.0, x, 3, y, 3, 3)
print(f"{total:.0f} {y[0]:.0f} {y[1]:.0f} {y[2]:.0f}")
