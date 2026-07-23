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

__all__ = ["compile", "Module", "OwnedArray", "LulangError"]


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
    if type_name == "f32":
        return ctypes.c_float
    if type_name == "f64":
        return ctypes.c_double
    if type_name in {"i64", "bool"} or type_name in enums:
        return ctypes.c_int64
    raise LulangError(f"unsupported manifest type {type_name!r}")


def _split_types(source: str) -> list[str]:
    values: list[str] = []
    depth = 0
    start = 0
    for index, character in enumerate(source):
        if character in "[(":
            depth += 1
        elif character in "])":
            depth -= 1
        elif character == "," and depth == 0:
            values.append(source[start:index])
            start = index + 1
    values.append(source[start:])
    return values


def _callback_signature(type_name: str) -> tuple[list[str], str]:
    if not type_name.startswith("c_fn[(") or not type_name.endswith("]"):
        raise LulangError(f"invalid callback type {type_name!r}")
    inner = type_name[len("c_fn[("):-1]
    marker = inner.find(")->")
    if marker < 0:
        raise LulangError(f"invalid callback type {type_name!r}")
    params = [] if marker == 0 else _split_types(inner[:marker])
    return params, inner[marker + 3:]


def _callback_ctype(type_name: str, enums: set[str]) -> type[ctypes._CFuncPtr]:
    params, ret = _callback_signature(type_name)
    raw_params: list[Any] = []
    for parameter in params:
        if parameter == "str":
            raw_params.extend([ctypes.c_void_p, ctypes.c_int64])
        elif parameter.startswith("c_ptr[") or parameter.startswith("c_fn["):
            raw_params.append(ctypes.c_void_p)
        else:
            raw_params.append(_scalar_ctype(parameter, enums))
    if ret == "()":
        raw_ret = None
    elif ret.startswith("c_ptr[") or ret.startswith("c_fn["):
        raw_ret = ctypes.c_void_p
    else:
        raw_ret = _scalar_ctype(ret, enums)
    return ctypes.CFUNCTYPE(raw_ret, *raw_params)


def _array_argument(
    value: Any, element: str, *, writable: bool
) -> tuple[Any, int, Any]:
    ctype = ctypes.c_double if element == "f64" else ctypes.c_int64

    try:
        import numpy  # type: ignore
    except ImportError:
        numpy = None
    if numpy is not None and isinstance(value, numpy.ndarray):
        expected = numpy.float64 if element == "f64" else numpy.int64
        if value.dtype != expected:
            raise TypeError(f"expected a {expected} NumPy array, got {value.dtype}")
        if not value.flags.c_contiguous or (writable and not value.flags.writeable):
            requirement = "writable and C-contiguous" if writable else "C-contiguous"
            raise TypeError(f"lulang array arguments must be {requirement}")
        return value.ctypes.data_as(ctypes.POINTER(ctype)), int(value.size), value

    if isinstance(value, ctypes.Array):
        return ctypes.cast(value, ctypes.POINTER(ctype)), len(value), value

    if isinstance(value, list):
        storage = (ctype * len(value))(*value)

        if writable:
            def copy_back() -> None:
                value[:] = storage[:]

            return ctypes.cast(storage, ctypes.POINTER(ctype)), len(value), (storage, copy_back)
        return ctypes.cast(storage, ctypes.POINTER(ctype)), len(value), storage

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


class OwnedArray:
    """A zero-copy scalar array result whose native allocation is explicitly owned."""

    def __init__(self, library: ctypes.CDLL, handle: int, element: str):
        self._library = library
        self._handle = ctypes.c_void_p(handle)
        self._ctype = _scalar_ctype(element, set())
        self._data = getattr(library, f"lu_owned_{element}_data")
        self._length = getattr(library, f"lu_owned_{element}_len")
        self._release = getattr(library, f"lu_owned_{element}_release")
        self._data.argtypes = [ctypes.c_void_p]
        self._data.restype = ctypes.POINTER(self._ctype)
        self._length.argtypes = [ctypes.c_void_p]
        self._length.restype = ctypes.c_int64
        self._release.argtypes = [ctypes.c_void_p]
        self._release.restype = None

    def __len__(self) -> int:
        return int(self._length(self._handle)) if self._handle else 0

    def __getitem__(self, index: int) -> Any:
        if index < 0:
            index += len(self)
        if index < 0 or index >= len(self):
            raise IndexError(index)
        return self._data(self._handle)[index]

    def __setitem__(self, index: int, value: Any) -> None:
        if index < 0:
            index += len(self)
        if index < 0 or index >= len(self):
            raise IndexError(index)
        self._data(self._handle)[index] = value

    def __iter__(self):
        for index in range(len(self)):
            yield self[index]

    def close(self) -> None:
        if self._handle:
            self._release(self._handle)
            self._handle = ctypes.c_void_p()

    def __enter__(self) -> "OwnedArray":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    def __del__(self) -> None:
        self.close()


class _Function:
    def __init__(
        self,
        library: ctypes.CDLL,
        spec: dict[str, Any],
        enums: set[str],
        records: dict[str, type[ctypes.Structure]],
        error_results: set[str],
    ):
        self.__name__ = spec["name"]
        self.__doc__ = (
            f"lulang export {self.__name__}("
            + ", ".join(f"{p['name']}: {p['type']}" for p in spec["params"])
            + f") -> {spec['ret']}"
        )
        self._spec = spec
        self._enums = enums
        self._records = records
        self._error_results = error_results
        self._library = library
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
            elif type_name.startswith("c_slice["):
                element = type_name[len("c_slice["):-1]
                argtypes.extend(
                    [ctypes.POINTER(_scalar_ctype(element, enums)), ctypes.c_int64]
                )
            elif type_name.startswith("c_mut_slice["):
                element = type_name[len("c_mut_slice["):-1]
                argtypes.extend(
                    [ctypes.POINTER(_scalar_ctype(element, enums)), ctypes.c_int64]
                )
            elif type_name.startswith("c_fn["):
                argtypes.append(_callback_ctype(type_name, enums))
            elif type_name in records:
                argtypes.append(records[type_name])
            else:
                argtypes.append(_scalar_ctype(type_name, enums))
        ret = spec["ret"]
        if ret == "str":
            argtypes.append(ctypes.POINTER(ctypes.c_int64))
        self._function.argtypes = argtypes
        if ret == "()":
            self._function.restype = None
        elif ret == "str":
            self._function.restype = ctypes.c_void_p
        elif ret.startswith("["):
            self._function.restype = ctypes.c_void_p
        elif ret.startswith("c_fn["):
            self._function.restype = _callback_ctype(ret, enums)
        elif ret in records:
            self._function.restype = records[ret]
        else:
            self._function.restype = _scalar_ctype(ret, enums)

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
                pointer, length, owner = _array_argument(
                    value, type_name[1:-1], writable=True
                )
                flattened.extend([pointer, length])
                keepalive.append(owner)
                if isinstance(owner, tuple):
                    copy_backs.append(owner[1])
            elif type_name.startswith("c_slice["):
                pointer, length, owner = _array_argument(
                    value, type_name[len("c_slice["):-1], writable=False
                )
                flattened.extend([pointer, length])
                keepalive.append(owner)
            elif type_name.startswith("c_mut_slice["):
                pointer, length, owner = _array_argument(
                    value, type_name[len("c_mut_slice["):-1], writable=True
                )
                flattened.extend([pointer, length])
                keepalive.append(owner)
                if isinstance(owner, tuple):
                    copy_backs.append(owner[1])
            elif type_name in self._records:
                record_type = self._records[type_name]
                if isinstance(value, record_type):
                    record = value
                elif isinstance(value, dict):
                    record = record_type(**value)
                else:
                    record = record_type(*value)
                flattened.append(record)
                keepalive.append(record)
            elif type_name.startswith("c_fn["):
                callback_type = _callback_ctype(type_name, self._enums)
                callback = value if isinstance(value, callback_type) else callback_type(value)
                flattened.append(callback)
                keepalive.append(callback)
            elif type_name == "bool":
                flattened.append(int(bool(value)))
            else:
                flattened.append(value)
        returned_length = ctypes.c_int64()
        if self._spec["ret"] == "str":
            flattened.append(ctypes.byref(returned_length))
        result = self._function(*flattened)
        for copy_back in copy_backs:
            copy_back()
        if self._spec["ret"] == "str":
            return ctypes.string_at(result, returned_length.value)
        if self._spec["ret"].startswith("["):
            return OwnedArray(self._library, result, self._spec["ret"][1:-1])
        if self._spec["ret"] in self._error_results:
            if result.status != 0:
                raise LulangError(f"Lulang error status {result.status}")
            return result.value
        if self._spec["ret"].startswith("c_fn["):
            # ctypes creates a fresh function-pointer object for the returned
            # address. Keep both it and any Python callbacks supplied to this
            # call alive for as long as the returned callable is reachable.
            returned_callback = result
            callback_owners = tuple(keepalive)

            def retained_callback(*callback_args: Any) -> Any:
                return returned_callback(*callback_args)

            retained_callback._lulang_callback = returned_callback
            retained_callback._lulang_keepalive = callback_owners
            return retained_callback
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
        record_specs = manifest.get("c_layout_records", {})
        records = {
            record_name: type(record_name, (ctypes.Structure,), {})
            for record_name in record_specs
        }
        defined: set[str] = set()

        def define_record(record_name: str) -> None:
            if record_name in defined:
                return
            field_types = []
            for field in record_specs[record_name]:
                type_name = field["type"]
                if type_name in records:
                    define_record(type_name)
                    ctype = records[type_name]
                else:
                    ctype = _scalar_ctype(type_name, enums)
                field_types.append((field["name"], ctype))
            records[record_name]._fields_ = field_types
            defined.add(record_name)

        for record_name in records:
            define_record(record_name)
        error_results = {
            record_name
            for record_name, fields in record_specs.items()
            if fields
            == [
                {"name": "status", "type": "i64"},
                {"name": "value", "type": "i64"},
            ]
        }
        self._exports = {
            spec["name"]: _Function(
                self._library, spec, enums, records, error_results
            )
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
