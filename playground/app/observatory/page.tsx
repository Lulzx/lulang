import type { Metadata } from "next";
import Link from "next/link";
import { benchmarks, repository, runtimes } from "./data";

export const metadata: Metadata = {
  title: "Benchmark observatory",
  description:
    "Reproducible lulang, C++, Rust, Julia, NumPy, and JavaScript benchmark results with source and assumptions.",
};

const toggles = [
  ["LU_MATH", "inline | call"],
  ["LU_IFCONV", "on | off"],
  ["LU_LICM", "on | off"],
  ["LU_SIMD", "on | off"],
  ["LU_LAYOUT", "soa | aos"],
];

export default function Observatory() {
  return (
    <main className="observatory-page">
      <nav className="nav" aria-label="Primary navigation">
        <Link className="wordmark" href="/" aria-label="lulang home">
          <span>lu</span><i>/</i>lang
        </Link>
        <div className="nav-links">
          <Link href="/#why">Overview</Link>
          <Link href="/#playground">Playground</Link>
          <a href="https://github.com/Lulzx/lulang">Source</a>
        </div>
        <Link className="nav-cta" href="/#playground">Run online</Link>
      </nav>

      <header className="observatory-hero">
        <div className="section-index"><span /> BENCHMARK OBSERVATORY</div>
        <h1>Numbers without source are not results.</h1>
        <p>
          These are median whole-process times in milliseconds. Initialization
          and output are included. Answers are checked before timings are
          accepted. Missing runtimes are not estimated.
        </p>
        <dl>
          <div><dt>Snapshot</dt><dd>2026-07-23</dd></div>
          <div><dt>Machine</dt><dd>Apple arm64 · macOS 26.6</dd></div>
          <div><dt>Runs</dt><dd>7 per command · median</dd></div>
          <div><dt>Source state</dt><dd>301bfee · dirty, recorded</dd></div>
        </dl>
      </header>

      <section className="benchmark-table-section" aria-labelledby="results-heading">
        <div className="observatory-section-heading">
          <div>
            <div className="section-index">01 / RESULTS</div>
            <h2 id="results-heading">Lower is better.</h2>
          </div>
          <a href={`${repository}benchmarks/observatory.tsv`}>Raw TSV ↗</a>
        </div>
        <div className="benchmark-table-wrap">
          <table className="benchmark-table">
            <thead>
              <tr>
                <th scope="col">Runtime</th>
                {benchmarks.map((benchmark) => (
                  <th scope="col" key={benchmark.name}>{benchmark.name}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {runtimes.map((runtime) => (
                <tr key={runtime}>
                  <th scope="row">{runtime}</th>
                  {benchmarks.map((benchmark) => (
                    <td key={benchmark.name} className={benchmark.results[runtime] === null ? "missing" : ""}>
                      {benchmark.results[runtime] ?? "—"}
                      {benchmark.results[runtime] && <small> ms</small>}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="benchmark-sources" aria-labelledby="sources-heading">
        <div className="observatory-section-heading">
          <div>
            <div className="section-index light">02 / SOURCE AND ASSUMPTIONS</div>
            <h2 id="sources-heading">The comparison is inspectable.</h2>
          </div>
        </div>
        <div className="source-ledger">
          {benchmarks.map((benchmark, index) => (
            <article key={benchmark.name}>
              <span>0{index + 1}</span>
              <div>
                <h3>{benchmark.name}</h3>
                <p>{benchmark.assumptions}</p>
              </div>
              <div className="source-links">
                {benchmark.sources.map((source) => (
                  <a key={source.path} href={`${repository}${source.path}`}>{source.label} ↗</a>
                ))}
                <a href={`${repository}${benchmark.llvm}`}>LLVM IR ↗</a>
              </div>
            </article>
          ))}
        </div>
      </section>

      <section className="ablation-section" aria-labelledby="ablation-heading">
        <div>
          <div className="section-index">03 / ABLATIONS</div>
          <h2 id="ablation-heading">Optimizations can be switched off.</h2>
          <p>
            The same source can be rerun with individual transformations
            disabled. Layout is explicit in the report: contiguous vectors for
            dot, value quaternions for slerp, and SoA for record-array kernels.
          </p>
          <a href={`${repository}experiments/RESULTS.md`}>Read the experiments ↗</a>
        </div>
        <dl className="toggle-list">
          {toggles.map(([name, values]) => (
            <div key={name}><dt>{name}</dt><dd>{values}</dd></div>
          ))}
        </dl>
      </section>

      <section className="embedded-proof" aria-labelledby="embedded-heading">
        <div className="embedded-proof-copy">
          <div className="section-index light">04 / EMBEDDED PROOF</div>
          <h2 id="embedded-heading">One function. One C symbol.</h2>
          <p>
            The notebook compiles a value-semantic quaternion kernel, loads the
            generated library with <code>pylulang</code>, checks its result
            against NumPy, and then measures both implementations.
          </p>
          <div className="source-links">
            <a href={`${repository}examples/embedded_slerp.lu`}>lulang source ↗</a>
            <a href={`${repository}examples/embedded_slerp.h`}>generated header ↗</a>
            <a href={`${repository}examples/embedded_slerp.json`}>ABI manifest ↗</a>
            <a href={`${repository}examples/lulang_embedded.ipynb`}>notebook ↗</a>
          </div>
        </div>
        <div className="embedded-header">
          <span>embedded_slerp.h</span>
          <pre><code>{`/* export fn slerp_checksum(count: i64): f64 */
double slerp_checksum(int64_t count);`}</code></pre>
        </div>
        <dl className="embedded-metrics">
          <div><dt>lulang</dt><dd>9.593 ms</dd></div>
          <div><dt>NumPy</dt><dd>62.425 ms</dd></div>
          <div className="speedup"><dt>Measured speedup</dt><dd>6.51×</dd></div>
          <div className="measurement-note"><dt>Method</dt><dd>2M slerps · 5 runs · median · compilation excluded · result checked</dd></div>
        </dl>
      </section>

      <section className="reproduce-section">
        <div className="section-index light">05 / REPRODUCE</div>
        <h2>Run the same measurement.</h2>
        <pre><code>python3 benchmarks/run_observatory.py --runs 7 --bootstrap</code></pre>
        <p>
          The runner builds the host and three-stage self-hosted compilers,
          checks numerical answers, records tool versions, and writes the table.
        </p>
      </section>

      <footer>
        <div>
          <div className="footer-mark">lu/</div>
          <h2>The source is part of the number.</h2>
        </div>
        <div className="footer-links">
          <Link href="/#playground">Open playground</Link>
          <a href="https://github.com/Lulzx/lulang/tree/main/benchmarks">Benchmark sources ↗</a>
          <Link href="/">lulang home</Link>
        </div>
      </footer>
    </main>
  );
}
