export type Benchmark = {
  name: string;
  assumptions: string;
  results: Record<string, string | null>;
  sources: { label: string; path: string }[];
  llvm: string;
};

export const runtimes = [
  "lulang AOT",
  "lulang JIT",
  "lulang selfhost",
  "C++ -O3",
  "C++ fast",
  "Rust",
  "Julia",
  "NumPy",
  "JavaScript",
];

export const benchmarks: Benchmark[] = [
  {
    name: "dot 2M × 20",
    assumptions:
      "Order-free sum; approximate floating point; contiguous f64 vectors; whole process.",
    results: {
      "lulang AOT": "64.053",
      "lulang JIT": "56.535",
      "lulang selfhost": "13.677",
      "C++ -O3": "27.815",
      "C++ fast": "11.902",
      Rust: "27.089",
      Julia: null,
      NumPy: "63.199",
      JavaScript: "44.622",
    },
    sources: [
      { label: "lulang", path: "corpus/bench_dot.lu" },
      { label: "C++", path: "corpus/bench_dot.cpp" },
      { label: "Rust", path: "corpus/bench_dot.rs" },
      { label: "Julia", path: "corpus/bench_dot.jl" },
      { label: "NumPy", path: "corpus/bench_dot.py" },
      { label: "JavaScript", path: "corpus/bench_dot.ts" },
    ],
    llvm: "benchmarks/ir/bench_dot.ll",
  },
  {
    name: "slerp 2M",
    assumptions:
      "Approximate floating point; value quaternions; NumPy uses a vectorized batch; whole process.",
    results: {
      "lulang AOT": "12.738",
      "lulang JIT": "36.039",
      "lulang selfhost": "12.096",
      "C++ -O3": "14.259",
      "C++ fast": "12.669",
      Rust: "14.583",
      Julia: null,
      NumPy: "135.866",
      JavaScript: "35.352",
    },
    sources: [
      { label: "lulang", path: "corpus/bench_slerp.lu" },
      { label: "C++", path: "corpus/bench_slerp.cpp" },
      { label: "Rust", path: "corpus/bench_slerp.rs" },
      { label: "Julia", path: "corpus/bench_slerp.jl" },
      { label: "NumPy", path: "corpus/bench_slerp.py" },
      { label: "JavaScript", path: "corpus/bench_slerp.ts" },
    ],
    llvm: "benchmarks/ir/bench_slerp.ll",
  },
];

export const repository = "https://github.com/Lulzx/lulang/blob/main/";
