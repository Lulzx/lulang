import importlib.util
import os
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("lulang_lsp", ROOT / "tools/lulang_lsp.py")
LSP = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(LSP)


class LspTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        os.environ["LULANG_BIN"] = str(ROOT / "target/release/lu")

    def test_diagnostics_use_non_executing_check_mode(self):
        self.assertEqual(LSP.diagnostics("main { print(1 / 0) }\n"), [])
        errors = LSP.diagnostics("main { print(missing) }\n")
        self.assertEqual(len(errors), 1)
        self.assertIn("unknown variable", errors[0]["message"])

    def test_symbols_and_formatting(self):
        source = "export fn twice(x:i64):i64{x*2}\nmain{print(twice(2))}\n"
        symbols = LSP.document_symbols(source)
        self.assertEqual([symbol["name"] for symbol in symbols], ["twice"])
        self.assertIn("export fn twice", LSP.format_document(source))


if __name__ == "__main__":
    unittest.main()
