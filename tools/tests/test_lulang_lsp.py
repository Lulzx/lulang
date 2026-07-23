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
        os.environ.setdefault("LULANG_BIN", str(ROOT / "target/release/lu"))

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

    def test_operator_navigation_property_lenses_and_single_property_runs(self):
        source = (
            "operator+ (a: i64) ⊕ (b: i64): i64 { a + b }\n"
            "property skipped(x: i64) { false }\n"
            "property selected(x: i64) { 2 ⊕ 3 == 5 }\n"
            "main { print(2 ⊕ 3) }\n"
        )
        operators = LSP.operator_declarations(source)
        self.assertEqual(operators[0]["glyph"], "⊕")
        row = source.splitlines()[3]
        target = LSP.operator_at(source, 3, row.index("⊕"))
        self.assertEqual(target["range"], operators[0]["range"])
        lenses = LSP.property_lenses(source, "file:///example.lu")
        self.assertEqual(
            [lens["command"]["arguments"][1] for lens in lenses],
            ["skipped", "selected"],
        )
        success, output = LSP.run_property(source, "selected", 11)
        self.assertTrue(success, output)
        self.assertIn("property selected ... ok (11 runs)", output)


if __name__ == "__main__":
    unittest.main()
