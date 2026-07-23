"""Execute the code cells in examples/lulang_embedded.ipynb without Jupyter."""

from __future__ import annotations

import json
from pathlib import Path


def main() -> None:
    notebook_path = Path(__file__).with_name("lulang_embedded.ipynb")
    notebook = json.loads(notebook_path.read_text())
    namespace = {"__name__": "__main__"}
    for index, cell in enumerate(notebook["cells"]):
        if cell["cell_type"] != "code":
            continue
        source = "".join(cell["source"])
        exec(compile(source, f"{notebook_path.name}:cell-{index + 1}", "exec"), namespace)


if __name__ == "__main__":
    main()
