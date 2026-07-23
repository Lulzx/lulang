# lulang website

The public landing page and browser interpreter for lulang.

## Development

Requires Node.js 22.13 or newer.

```bash
npm install
npm run dev
npm test
```

The playground implements a small, intentionally local subset of the language.
It is used to try scalar expressions, arrays, functions, `sum`, and value
semantics without installing the native compiler.
