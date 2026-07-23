"""Python bindings for lulang's generated C ABI."""

from __future__ import annotations

import ctypes
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Any

__all__ = ["compile", "Module", "LulangError"]


class LulangError(RuntimeError):
    pass


def _find_compiler(explicit: str | os.PathLike[str] | None) -> str:
    candidates = [
        explicit,
        os.environ.get("LULANG_BIN"),
        shutil.which("lu"),
        Path(__file__).resolve().parents[3] / "target" / "release" / "lu",
    ]
    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(candidate)
    raise LulangError(
        "cannot find `lu`; pass lu=..., set LULANG_BIN, or put it on PATH"
    )


def _scalar_ctype(type_name: str, enums: set[str]) -> type[ctypes._SimpleCData]:
    if type_name == "f64":
        return ctypes.c_double
    if type_name in {"i64", "bool"} or type_name in enums:
        return ctypes.c_int64
    raise LulangError(f"unsupported manifest type {type_name!r}")


def _array_argument(value: Any, element: str) -> tuple[Any, int, Any]:
    ctype = ctypes.c_double if element == "f64" else ctypes.c_int64

    try:
        import numpy  # type: ignore
    except ImportError:
        numpy = None
    if numpy is not None and isinstance(value, numpy.ndarray):
        expected = numpy.float64 if element == "f64" else numpy.int64
        if value.dtype != expected:
            raise TypeError(f"expected a {expected} NumPy array, got {value.dtype}")
        if not value.flags.c_contiguous or not value.flags.writeable:
            raise TypeError("lulang array arguments must be writable and C-contiguous")
        return value.ctypes.data_as(ctypes.POINTER(ctype)), int(value.size), value

    if isinstance(value, ctypes.Array):
        return ctypes.cast(value, ctypes.POINTER(ctype)), len(value), value

    if isinstance(value, list):
        storage = (ctype * len(value))(*value)

        def copy_back() -> None:
            value[:] = storage[:]

        return ctypes.cast(storage, ctypes.POINTER(ctype)), len(value), (storage, copy_back)

    try:
        view = memoryview(value)
    except TypeError as error:
        raise TypeError(
            f"expected a list, ctypes array, writable buffer, or NumPy array for [{element}]"
        ) from error
    expected_format = "d" if element == "f64" else "q"
    if view.readonly or not view.c_contiguous or view.itemsize != ctypes.sizeof(ctype):
        raise TypeError("lulang array buffers must be writable, contiguous, and 64-bit")
    if view.format not in {expected_format, f"@{expected_format}", f"={expected_format}"}:
        raise TypeError(f"expected a [{element}] compatible buffer, got format {view.format!r}")
    storage = (ctype * view.shape[0]).from_buffer(view)
    return ctypes.cast(storage, ctypes.POINTER(ctype)), len(storage), storage


class _Function:
    def __init__(self, library: ctypes.CDLL, spec: dict[str, Any], enums: set[str]):
        self.__name__ = spec["name"]
        self.__doc__ = (
            f"lulang export {self.__name__}("
            + ", ".join(f"{p['name']}: {p['type']}" for p in spec["params"])
            + f") -> {spec['ret']}"
        )
        self._spec = spec
        self._enums = enums
        self._function = getattr(library, self.__name__)
        argtypes: list[Any] = []
        for parameter in spec["params"]:
            type_name = parameter["type"]
            if type_name == "str":
                argtypes.extend([ctypes.c_void_p, ctypes.c_int64])
            elif type_name.startswith("["):
                element = type_name[1:-1]
                argtypes.extend(
                    [ctypes.POINTER(_scalar_ctype(element, enums)), ctypes.c_int64]
                )
            else:
                argtypes.append(_scalar_ctype(type_name, enums))
        self._function.argtypes = argtypes
        ret = spec["ret"]
        self._function.restype = None if ret == "()" else _scalar_ctype(ret, enums)

    def __call__(self, *args: Any) -> Any:
        if len(args) != len(self._spec["params"]):
            raise TypeError(
                f"{self.__name__} expects {len(self._spec['params'])} arguments, got {len(args)}"
            )
        flattened: list[Any] = []
        keepalive: list[Any] = []
        copy_backs: list[Any] = []
        for value, parameter in zip(args, self._spec["params"]):
            type_name = parameter["type"]
            if type_name == "str":
                encoded = value.encode() if isinstance(value, str) else bytes(value)
                storage = ctypes.create_string_buffer(encoded)
                flattened.extend([ctypes.cast(storage, ctypes.c_void_p), len(encoded)])
                keepalive.append(storage)
            elif type_name.startswith("["):
                pointer, length, owner = _array_argument(value, type_name[1:-1])
                flattened.extend([pointer, length])
                keepalive.append(owner)
                if isinstance(owner, tuple):
                    copy_backs.append(owner[1])
            elif type_name == "bool":
                flattened.append(int(bool(value)))
            else:
                flattened.append(value)
        result = self._function(*flattened)
        for copy_back in copy_backs:
            copy_back()
        return bool(result) if self._spec["ret"] == "bool" else result


class Module:
    """A loaded lulang shared library. Keep this object alive while using exports."""

    def __init__(self, directory: tempfile.TemporaryDirectory[str], name: str):
        self._directory = directory
        root = Path(directory.name)
        manifest = json.loads((root / f"{name}.json").read_text())
        extension = ".dylib" if os.uname().sysname == "Darwin" else ".so"
        self._library = ctypes.CDLL(str(root / f"lib{name}{extension}"))
        self.manifest = manifest
        enums = set(manifest.get("enums", {}))
        self._exports = {
            spec["name"]: _Function(self._library, spec, enums)
            for spec in manifest["exports"]
        }

    def __getattr__(self, name: str) -> Any:
        try:
            return self._exports[name]
        except KeyError as error:
            raise AttributeError(name) from error

    def __dir__(self) -> list[str]:
        return sorted(set(super().__dir__()) | set(self._exports))

    def close(self) -> None:
        self._directory.cleanup()

    def __enter__(self) -> "Module":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()


def compile(
    source: str | os.PathLike[str],
    *,
    name: str = "module",
    lu: str | os.PathLike[str] | None = None,
) -> Module:
    """Compile lulang source (or a `.lu` path) and return its exported functions."""

    compiler = _find_compiler(lu)
    directory: tempfile.TemporaryDirectory[str] = tempfile.TemporaryDirectory(
        prefix="pylulang-"
    )
    root = Path(directory.name)
    source_path = Path(source)
    if isinstance(source, os.PathLike) or ("\n" not in str(source) and source_path.is_file()):
        text = source_path.read_text()
    else:
        text = str(source)
    input_path = root / f"{name}.lu"
    input_path.write_text(text)
    command = [
        compiler,
        "build",
        "--lib",
        "--shared",
        "-o",
        str(root / name),
        str(input_path),
    ]
    result = subprocess.run(command, text=True, capture_output=True)
    if result.returncode:
        directory.cleanup()
        raise LulangError(result.stderr.strip() or result.stdout.strip())
    return Module(directory, name)
