const vscode = require("vscode");
const cp = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");

const keywords = [
  "main", "fn", "export", "extern", "type", "enum", "property", "operator",
  "let", "var", "inout", "return", "if", "else", "for", "in", "while",
  "true", "false", "and", "or", "not",
];
const builtins = {
  arr: "arr(n, initial) -> [T]",
  len: "len(value) -> i64",
  sum: "sum(i in lo..hi) expression",
  print: "print(values...)",
  sqrt: "sqrt(x: f64) -> f64",
  sin: "sin(x: f64) -> f64",
  cos: "cos(x: f64) -> f64",
};

function compiler() {
  return vscode.workspace.getConfiguration("lulang").get("compilerPath", "lu");
}

function withTemp(document, action) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "lulang-vscode-"));
  const source = path.join(directory, "document.lu");
  fs.writeFileSync(source, document.getText());
  try {
    return action(source);
  } finally {
    fs.rmSync(directory, { recursive: true, force: true });
  }
}

function activate(context) {
  const diagnostics = vscode.languages.createDiagnosticCollection("lulang");
  context.subscriptions.push(diagnostics);

  const validate = (document) => {
    if (document.languageId !== "lulang") return;
    withTemp(document, (source) => {
      cp.execFile(compiler(), ["check", source], (error, stdout, stderr) => {
        if (!error) {
          diagnostics.set(document.uri, []);
          return;
        }
        const end = document.lineAt(Math.max(0, document.lineCount - 1)).range.end;
        const diagnostic = new vscode.Diagnostic(
          new vscode.Range(new vscode.Position(0, 0), end),
          (stderr || stdout).trim().replace(/^error: /, ""),
          vscode.DiagnosticSeverity.Error
        );
        diagnostic.source = "lulang";
        diagnostics.set(document.uri, [diagnostic]);
      });
    });
  };
  context.subscriptions.push(
    vscode.workspace.onDidOpenTextDocument(validate),
    vscode.workspace.onDidChangeTextDocument((event) => validate(event.document)),
    vscode.workspace.onDidSaveTextDocument(validate),
    vscode.workspace.onDidCloseTextDocument((document) => diagnostics.delete(document.uri))
  );
  vscode.workspace.textDocuments.forEach(validate);

  context.subscriptions.push(vscode.languages.registerDocumentFormattingEditProvider(
    "lulang",
    {
      provideDocumentFormattingEdits(document) {
        return withTemp(document, (source) => {
          cp.execFileSync(compiler(), ["fmt", source]);
          const formatted = fs.readFileSync(source, "utf8");
          const end = document.lineAt(document.lineCount - 1).range.end;
          return [vscode.TextEdit.replace(new vscode.Range(new vscode.Position(0, 0), end), formatted)];
        });
      },
    }
  ));

  context.subscriptions.push(vscode.languages.registerCompletionItemProvider("lulang", {
    provideCompletionItems() {
      return [
        ...keywords.map((name) => new vscode.CompletionItem(name, vscode.CompletionItemKind.Keyword)),
        ...Object.entries(builtins).map(([name, detail]) => {
          const item = new vscode.CompletionItem(name, vscode.CompletionItemKind.Function);
          item.detail = detail;
          return item;
        }),
      ];
    },
  }));

  context.subscriptions.push(vscode.languages.registerHoverProvider("lulang", {
    provideHover(document, position) {
      const range = document.getWordRangeAtPosition(position);
      const word = range && document.getText(range);
      return word && builtins[word]
        ? new vscode.Hover(new vscode.MarkdownString(`\`\`\`lu\n${builtins[word]}\n\`\`\``))
        : undefined;
    },
  }));
}

function deactivate() {}

module.exports = { activate, deactivate };
