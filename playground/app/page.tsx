import type { Metadata } from "next";
import { Playground } from "./playground";

export const metadata: Metadata = {
  title: { absolute: "lulang — a language for numerical computing" },
  description:
    "lulang is a small language for numerical computing with value semantics, native code generation, and C and Python interfaces.",
};

const boundaryCode = `export fn saxpy(a: f64, x: [f64], inout y: [f64], n: i64) {
  for i in 0..n {
    y[i] = a * x[i] + y[i]
  }
}`;

export default function Home() {
  return (
    <main>
      <nav className="nav" aria-label="Primary navigation">
        <a className="wordmark" href="#top" aria-label="lulang home">
          <span>lu</span><i>/</i>lang
        </a>
        <div className="nav-links">
          <a href="#why">Overview</a>
          <a href="#playground">Playground</a>
          <a href="https://github.com/Lulzx/lulang">Source</a>
        </div>
        <a className="nav-cta" href="#playground">Run online</a>
      </nav>

      <section className="hero" id="top">
        <div className="hero-copy">
          <div className="eyebrow"><span /> LULANG PROGRAMMING LANGUAGE</div>
          <h1>lulang</h1>
          <h2>A small language for numerical computing.</h2>
          <p className="hero-lede">
            lulang compiles numerical functions to native code. Programs use
            value semantics, and floating-point expressions may be reassociated.
            Compiled functions can be called from C or Python.
          </p>
          <div className="hero-actions">
            <a className="button-primary" href="#playground">Run online</a>
            <a className="button-text" href="https://github.com/Lulzx/lulang">
              Source code <span aria-hidden="true">↗</span>
            </a>
          </div>
          <dl className="proof-strip">
            <div><dt>Interpreter</dt><dd>reference implementation</dd></div>
            <div><dt>JIT + LLVM</dt><dd>native code generation</dd></div>
            <div><dt>C · Python</dt><dd>generated interfaces</dd></div>
          </dl>
        </div>
        <div className="hero-code" aria-label="lulang export example">
          <div className="code-chrome">
            <span>kernel_saxpy.lu</span>
            <span className="code-state">BOUNDARY ABI</span>
          </div>
          <pre><code>{boundaryCode}</code></pre>
          <div className="code-result">
            <span className="result-mark">→</span>
            <div><b>libkernel_saxpy</b><small>.a · .dylib/.so · .h · .json</small></div>
          </div>
          <div className="orbit orbit-one" />
          <div className="orbit orbit-two" />
        </div>
      </section>

      <div className="ticker" aria-hidden="true">
        <div>
          <span>NO BORROW CHECKER</span><b>◆</b><span>NO ALIASING</span><b>◆</b>
          <span>COMPILER-OWNED LAYOUT</span><b>◆</b><span>APPROXIMATE FP BY CONTRACT</span><b>◆</b>
          <span>NO BORROW CHECKER</span><b>◆</b><span>NO ALIASING</span><b>◆</b>
          <span>COMPILER-OWNED LAYOUT</span><b>◆</b><span>APPROXIMATE FP BY CONTRACT</span>
        </div>
      </div>

      <section className="argument" id="why">
        <div className="section-index">01 / OVERVIEW</div>
        <div className="argument-heading">
          <h2>Main properties</h2>
          <p>
            lulang is intended for small numerical kernels. Its semantics make
            common numerical optimizations legal without unsafe compiler flags.
          </p>
        </div>
        <div className="principles">
          <article>
            <span>01</span>
            <h3>Numerical semantics</h3>
            <p>Floating-point operations may be reassociated. Approximate equality is a language operator.</p>
          </article>
          <article>
            <span>02</span>
            <h3>Value semantics</h3>
            <p>Arrays and records are values. A function cannot retain an alias to its caller&apos;s data.</p>
          </article>
          <article>
            <span>03</span>
            <h3>C interface</h3>
            <p><code>lu build --lib</code> creates a library, a C header, and a machine-readable manifest.</p>
          </article>
        </div>
      </section>

      <section className="workflow">
        <div className="workflow-copy">
          <div className="section-index">02 / USE FROM AN EXISTING PROGRAM</div>
          <h2>Compile one function.</h2>
          <p>
            A lulang library can be added to an existing C or Python program.
            The application and its data structures do not need to be rewritten.
          </p>
        </div>
        <ol className="workflow-steps">
          <li><span>01</span><div><b>Write a function</b><small>Use scalar, array, and record types to express the calculation.</small></div></li>
          <li><span>02</span><div><b>Compile a library</b><small><code>lu build --lib --shared file.lu</code> creates the native library and interface files.</small></div></li>
          <li><span>03</span><div><b>Call the function</b><small>Include the generated C header, or load the manifest with <code>pylulang</code>.</small></div></li>
        </ol>
      </section>

      <section className="playground-section" id="playground">
        <div className="playground-intro">
          <div className="section-index light">03 / ONLINE INTERPRETER</div>
          <h2>Try lulang</h2>
          <p>Edit the example and press Run. The interpreter runs locally in this page.</p>
        </div>
        <Playground />
      </section>

      <section className="tiers">
        <div className="section-index">04 / IMPLEMENTATIONS</div>
        <div className="tier-track">
          <div><span>REFERENCE</span><b>CFG interpreter</b></div>
          <i>→</i><div><span>DEVELOP</span><b>Cranelift JIT</b></div>
          <i>→</i><div><span>SHIP</span><b>LLVM AOT</b></div>
          <i>→</i><div><span>ASCEND</span><b>Self-hosted compiler</b></div>
        </div>
      </section>

      <footer>
        <div>
          <div className="footer-mark">lu/</div>
          <h2>lulang is experimental software.</h2>
        </div>
        <div className="footer-links">
          <a href="#playground">Open playground</a>
          <a href="https://github.com/Lulzx/lulang">GitHub ↗</a>
          <a href="https://github.com/Lulzx/lulang/blob/main/ROADMAP.md">Roadmap ↗</a>
        </div>
        <p className="footnote">Source code, language notes, tests, and reproducible benchmarks are available in the repository.</p>
      </footer>
    </main>
  );
}
