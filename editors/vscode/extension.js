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

function propertyDeclarations(document) {
  const declarations = [];
  document.getText().split(/\r?\n/).forEach((line, lineNumber) => {
    const match = line.match(/^\s*property\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (!match) return;
    const character = line.indexOf(match[1]);
    declarations.push({
      name: match[1],
      range: new vscode.Range(lineNumber, character, lineNumber, character + match[1].length),
    });
  });
  return declarations;
}

function operatorDeclarations(document) {
  const declarations = [];
  document.getText().split(/\r?\n/).forEach((line, lineNumber) => {
    const infix = line.match(/^\s*operator\S+\s+\([^)]*\)\s*([^\w\s(]+)\s*\(/u);
    const circumfix = line.match(/^\s*operator\s+([^\s(]+)\([^)]*\)([^\s:]+)/u);
    if (infix) {
      const character = line.indexOf(infix[1], line.indexOf(")"));
      declarations.push({
        aliases: [infix[1]],
        signature: line.split("{", 1)[0].trim(),
        range: new vscode.Range(lineNumber, character, lineNumber, character + infix[1].length),
      });
    } else if (circumfix) {
      const character = line.indexOf(circumfix[1], line.indexOf("operator") + 8);
      declarations.push({
        aliases: [circumfix[1], circumfix[2]],
        signature: line.split("{", 1)[0].trim(),
        range: new vscode.Range(lineNumber, character, lineNumber, character + circumfix[1].length),
      });
    }
  });
  return declarations;
}

function operatorAt(document, position) {
  const line = document.lineAt(position.line).text;
  for (const declaration of operatorDeclarations(document)) {
    for (const alias of declaration.aliases) {
      let at = line.indexOf(alias);
      while (at >= 0) {
        if (at <= position.character && position.character < at + alias.length) return declaration;
        at = line.indexOf(alias, at + alias.length);
      }
    }
  }
  return undefined;
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
      const operator = operatorAt(document, position);
      const detail = operator ? operator.signature : word && builtins[word];
      return detail
        ? new vscode.Hover(new vscode.MarkdownString(`\`\`\`lu\n${detail}\n\`\`\``))
        : undefined;
    },
  }));

  context.subscriptions.push(vscode.languages.registerDefinitionProvider("lulang", {
    provideDefinition(document, position) {
      const operator = operatorAt(document, position);
      if (operator) return new vscode.Location(document.uri, operator.range);
      const wordRange = document.getWordRangeAtPosition(position);
      if (!wordRange) return undefined;
      const word = document.getText(wordRange);
      const lines = document.getText().split(/\r?\n/);
      for (let lineNumber = 0; lineNumber < lines.length; lineNumber++) {
        const match = lines[lineNumber].match(
          /^\s*(?:(?:export|extern)(?:\s+"[^"]*")?\s+)?(?:fn|type|enum|property)\s+([A-Za-z_][A-Za-z0-9_]*)/
        );
        if (match && match[1] === word) {
          const character = lines[lineNumber].indexOf(word);
          return new vscode.Location(
            document.uri,
            new vscode.Range(lineNumber, character, lineNumber, character + word.length)
          );
        }
      }
      return undefined;
    },
  }));

  context.subscriptions.push(vscode.languages.registerCodeLensProvider("lulang", {
    provideCodeLenses(document) {
      return propertyDeclarations(document).map(({ name, range }) => new vscode.CodeLens(range, {
        title: "Run property (100 cases)",
        command: "lulang.runProperty",
        arguments: [document.uri, name, range],
      }));
    },
  }));

  context.subscriptions.push(vscode.commands.registerCommand(
    "lulang.runProperty",
    async (uri, name, range) => {
      const document = vscode.workspace.textDocuments.find((item) => item.uri.toString() === uri.toString())
        || await vscode.workspace.openTextDocument(uri);
      try {
        const output = withTemp(document, (source) => cp.execFileSync(
          compiler(),
          ["test", "--runs", "100", "--property", name, source],
          { encoding: "utf8" }
        ));
        diagnostics.set(document.uri, []);
        vscode.window.showInformationMessage(output.trim());
      } catch (error) {
        const output = `${error.stdout || ""}\n${error.stderr || ""}`.trim();
        const diagnostic = new vscode.Diagnostic(
          range,
          output || String(error),
          vscode.DiagnosticSeverity.Error
        );
        diagnostic.source = "lulang property";
        diagnostics.set(document.uri, [diagnostic]);
        vscode.window.showErrorMessage(output || String(error));
      }
    }
  ));
}

function deactivate() {}

module.exports = { activate, deactivate };
