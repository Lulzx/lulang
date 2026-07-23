"use client";

import { useMemo, useState } from "react";

type Token = { value: string; kind: "number" | "string" | "word" | "symbol" };
type Env = Record<string, unknown>;

const examples = {
  "Dot product": `fn dot(x: [f64], y: [f64], n: i64): f64 {
  return sum(i in 0..n) x[i] * y[i]
}

main {
  let x = [1.0, 2.0, 3.0, 4.0]
  let y = [4.0, 3.0, 2.0, 1.0]
  print("dot", dot(x, y, len(x)))
}`,
  "Value semantics": `fn touch(data: [i64]) {
  data[0] = 99
}

main {
  var values = arr(3, 0)
  values[0] = 7
  touch(values)
  print("outside", values[0])
}`,
  "Order-free sum": `main {
  let energy = sum(i in 0..1000) sin(float(i) * 0.01) * sin(float(i) * 0.01)
  print("energy", energy)
}`,
};

function lex(source: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;
  while (i < source.length) {
    if (/\s/.test(source[i])) { i++; continue; }
    if (source.slice(i, i + 2) === "//") {
      while (i < source.length && source[i] !== "\n") i++;
      continue;
    }
    if (source[i] === '"') {
      let value = ""; i++;
      while (i < source.length && source[i] !== '"') {
        if (source[i] === "\\" && i + 1 < source.length) {
          i++;
          value += ({ n: "\n", t: "\t" } as Record<string, string>)[source[i]] ?? source[i];
        } else value += source[i];
        i++;
      }
      if (source[i++] !== '"') throw new Error("unterminated string");
      tokens.push({ value, kind: "string" });
      continue;
    }
    const number = source.slice(i).match(/^(?:\d+(?:\.\d+)?|\.\d+)(?:[eE][+-]?\d+)?/);
    if (number) {
      tokens.push({ value: number[0], kind: "number" }); i += number[0].length; continue;
    }
    const word = source.slice(i).match(/^[A-Za-z_][A-Za-z0-9_]*/);
    if (word) {
      tokens.push({ value: word[0], kind: "word" }); i += word[0].length; continue;
    }
    const pair = source.slice(i, i + 2);
    if (["..", "==", "!=", "<=", ">=", "~="].includes(pair)) {
      tokens.push({ value: pair, kind: "symbol" }); i += 2; continue;
    }
    tokens.push({ value: source[i++], kind: "symbol" });
  }
  tokens.push({ value: "<eof>", kind: "symbol" });
  return tokens;
}

class BrowserInterpreter {
  tokens: Token[];
  at = 0;
  functions: Record<string, { params: string[]; body: Token[] }> = {};
  output: string[] = [];
  scopes: Env[] = [{}];
  returning = false;
  returnValue: unknown;

  constructor(source: string) { this.tokens = lex(source); }
  peek(): Token;
  peek(value: string): boolean;
  peek(value?: string): Token | boolean {
    return value ? this.tokens[this.at].value === value : this.tokens[this.at];
  }
  take(value?: string) {
    const token = this.tokens[this.at++];
    if (value && token.value !== value) throw new Error(`expected '${value}', found '${token.value}'`);
    return token;
  }
  cloneValue(value: unknown): unknown { return Array.isArray(value) ? value.map((item) => this.cloneValue(item)) : value; }
  get(name: string): unknown {
    for (let i = this.scopes.length - 1; i >= 0; i--) if (name in this.scopes[i]) return this.scopes[i][name];
    throw new Error(`unknown variable '${name}'`);
  }
  set(name: string, value: unknown) {
    for (let i = this.scopes.length - 1; i >= 0; i--) if (name in this.scopes[i]) { this.scopes[i][name] = value; return; }
    throw new Error(`unknown variable '${name}'`);
  }
  skipType() {
    if (!this.peek(":")) return;
    this.take(":");
    let depth = 0;
    while (!(depth === 0 && ["=", "{", ",", ")"].includes(this.peek().value))) {
      if (this.peek("[")) depth++;
      if (this.peek("]")) depth--;
      this.take();
    }
  }
  captureBlock(): Token[] {
    this.take("{"); const start = this.at; let depth = 1;
    while (depth) { if (this.peek("{")) depth++; if (this.peek("}")) depth--; this.at++; }
    return this.tokens.slice(start, this.at - 1).concat({ value: "<eof>", kind: "symbol" });
  }
  run() {
    while (!this.peek("<eof>")) {
      if (this.peek("export")) this.take();
      if (this.peek("fn")) this.parseFunction();
      else if (this.peek("main")) {
        this.take(); const body = this.captureBlock(); this.execute(body, this.scopes[0]);
      } else throw new Error(`unexpected top-level token '${this.peek().value}'`);
    }
    return this.output.join("\n");
  }
  parseFunction() {
    this.take("fn"); const name = this.take().value; this.take("("); const params: string[] = [];
    while (!this.peek(")")) {
      if (this.peek("inout")) this.take();
      params.push(this.take().value); this.skipType();
      if (this.peek(",")) this.take(",");
    }
    this.take(")"); this.skipType(); const bodyTokens = this.captureBlock();
    this.functions[name] = { params, body: bodyTokens };
  }
  execute(tokens: Token[], scope: Env) {
    const previous = { tokens: this.tokens, at: this.at };
    this.tokens = tokens; this.at = 0; this.scopes.push(scope);
    while (!this.peek("<eof>") && !this.returning) this.statement();
    this.scopes.pop(); this.tokens = previous.tokens; this.at = previous.at;
  }
  statement() {
    if (this.peek("let") || this.peek("var")) {
      this.take(); const name = this.take().value; this.skipType(); this.take("=");
      this.scopes[this.scopes.length - 1][name] = this.cloneValue(this.expression()); return;
    }
    if (this.peek("return")) {
      this.take(); this.returnValue = this.expression(); this.returning = true; return;
    }
    if (this.peek("for")) {
      this.take(); const name = this.take().value; this.take("in"); const lo = Number(this.expression());
      this.take(".."); const hi = Number(this.expression()); const body = this.captureBlock();
      for (let i = lo; i < hi && !this.returning; i++) this.execute(body, { [name]: i });
      return;
    }
    const mark = this.at;
    if (this.peek().kind === "word") {
      const name = this.take().value;
      if (this.peek("=")) { this.take(); this.set(name, this.expression()); return; }
      if (this.peek("[")) {
        this.take(); const index = Number(this.expression()); this.take("]");
        if (this.peek("=")) {
          this.take(); const array = this.get(name) as unknown[]; array[index] = this.expression(); return;
        }
      }
    }
    this.at = mark; this.expression();
  }
  expression(min = 0): unknown {
    let left = this.prefix();
    const precedence: Record<string, number> = { or: 1, and: 2, "==": 3, "!=": 3, "~=": 3, "<": 4, "<=": 4, ">": 4, ">=": 4, "+": 5, "-": 5, "*": 6, "/": 6, "%": 6 };
    while ((precedence[this.peek().value] ?? -1) >= min) {
      const op = this.take().value; const right = this.expression(precedence[op] + 1);
      left = this.binary(op, left, right);
    }
    return left;
  }
  prefix(): unknown {
    if (this.peek("-")) { this.take(); return -Number(this.prefix()); }
    if (this.peek("not")) { this.take(); return !this.prefix(); }
    if (this.peek("sum")) {
      this.take(); this.take("("); const name = this.take().value; this.take("in");
      const lo = Number(this.expression()); this.take(".."); const hi = Number(this.expression()); this.take(")");
      const bodyStart = this.at; let total = 0;
      for (let i = lo; i < hi; i++) { this.scopes.push({ [name]: i }); this.at = bodyStart; total += Number(this.expression()); this.scopes.pop(); }
      return total;
    }
    let value: unknown;
    const token = this.take();
    if (token.kind === "number") value = Number(token.value);
    else if (token.kind === "string") value = token.value;
    else if (token.value === "true" || token.value === "false") value = token.value === "true";
    else if (token.value === "(") { value = this.expression(); this.take(")"); }
    else if (token.value === "[") {
      const items: unknown[] = []; while (!this.peek("]")) { items.push(this.expression()); if (this.peek(",")) this.take(); }
      this.take("]"); value = items;
    } else if (this.peek("(")) value = this.call(token.value);
    else value = this.get(token.value);
    while (this.peek("[")) { this.take(); value = (value as unknown[])[Number(this.expression())]; this.take("]"); }
    return value;
  }
  call(name: string): unknown {
    this.take("("); const args: unknown[] = [];
    while (!this.peek(")")) { args.push(this.expression()); if (this.peek(",")) this.take(); }
    this.take(")");
    if (name === "print") { this.output.push(args.map((v) => typeof v === "number" ? Number(v.toPrecision(12)).toString() : String(v)).join(" ")); return null; }
    const builtins: Record<string, (...values: unknown[]) => unknown> = {
      arr: (n, value) => Array.from({ length: Number(n) }, () => this.cloneValue(value)),
      len: (value) => (value as { length: number }).length,
      float: Number, int: (value) => Math.trunc(Number(value)),
      sqrt: (value) => Math.sqrt(Number(value)), sin: (value) => Math.sin(Number(value)),
      cos: (value) => Math.cos(Number(value)), abs: (value) => Math.abs(Number(value)),
    };
    if (builtins[name]) return builtins[name](...args);
    const fn = this.functions[name]; if (!fn) throw new Error(`unknown function '${name}'`);
    const scope: Env = {}; fn.params.forEach((param, i) => scope[param] = this.cloneValue(args[i]));
    const oldReturning = this.returning; const oldValue = this.returnValue;
    this.returning = false; this.returnValue = null;
    this.execute(fn.body, scope); const result = this.returnValue;
    this.returning = oldReturning; this.returnValue = oldValue; return result;
  }
  binary(op: string, a: unknown, b: unknown): unknown {
    if (op === "+") return Number(a) + Number(b); if (op === "-") return Number(a) - Number(b);
    if (op === "*") return Number(a) * Number(b); if (op === "/") return Number(a) / Number(b);
    if (op === "%") return Number(a) % Number(b); if (op === "==") return a === b;
    if (op === "!=") return a !== b; if (op === "~=") return Math.abs(Number(a) - Number(b)) <= Math.max(Math.abs(Number(a)), Math.abs(Number(b))) * 2 ** -40 + 2 ** -100;
    if (op === "<") return Number(a) < Number(b); if (op === "<=") return Number(a) <= Number(b);
    if (op === ">") return Number(a) > Number(b); if (op === ">=") return Number(a) >= Number(b);
    if (op === "and") return Boolean(a && b); if (op === "or") return Boolean(a || b);
  }
}

export function Playground() {
  const first = Object.keys(examples)[0] as keyof typeof examples;
  const [selected, setSelected] = useState<keyof typeof examples>(first);
  const [source, setSource] = useState(examples[first]);
  const [output, setOutput] = useState("Ready. Choose an example or write a kernel.");
  const [state, setState] = useState<"idle" | "running" | "ok" | "error">("idle");
  const lines = useMemo(() => source.split("\n").length, [source]);

  function choose(name: keyof typeof examples) { setSelected(name); setSource(examples[name]); setOutput("Ready."); setState("idle"); }
  function run() {
    setState("running"); setOutput("Interpreting…");
    requestAnimationFrame(() => {
      try { setOutput(new BrowserInterpreter(source).run() || "(program completed with no output)"); setState("ok"); }
      catch (error) { setOutput(error instanceof Error ? error.message : String(error)); setState("error"); }
    });
  }

  return (
    <div className="playground-shell">
      <div className="example-rail" aria-label="Examples">
        <span>EXAMPLES</span>
        {(Object.keys(examples) as (keyof typeof examples)[]).map((name, index) => (
          <button key={name} onClick={() => choose(name)} className={selected === name ? "active" : ""}>
            <i>0{index + 1}</i>{name}
          </button>
        ))}
        <small>Core subset interpreter<br />Runs on this device</small>
      </div>
      <div className="editor-pane">
        <div className="panel-bar">
          <span><i className={`status-dot ${state}`} /> main.lu</span>
          <button onClick={() => navigator.clipboard?.writeText(source)}>COPY</button>
        </div>
        <div className="editor-wrap">
          <div className="line-numbers" aria-hidden="true">{Array.from({ length: lines }, (_, i) => <span key={i}>{i + 1}</span>)}</div>
          <textarea aria-label="lulang source editor" spellCheck={false} value={source} onChange={(event) => setSource(event.target.value)} />
        </div>
      </div>
      <div className="output-pane">
        <div className="panel-bar"><span>OUTPUT</span><span className={`run-state ${state}`}>{state === "running" ? "RUNNING" : state === "error" ? "ERROR" : state === "ok" ? "COMPLETE" : "IDLE"}</span></div>
        <pre aria-live="polite">{output}</pre>
        <button className="run-button" onClick={run} disabled={state === "running"}>
          <span>{state === "running" ? "Interpreting" : "Run program"}</span><kbd>CMD + ENTER</kbd>
        </button>
      </div>
    </div>
  );
}
