import assert from "node:assert/strict";
import test from "node:test";

async function render(path = "/") {
  const workerUrl = new URL("../dist/server/index.js", import.meta.url);
  workerUrl.searchParams.set("test", `${process.pid}-${Date.now()}`);
  const { default: worker } = await import(workerUrl.href);

  return worker.fetch(
    new Request(`http://localhost${path}`, {
      headers: { accept: "text/html" },
    }),
    {
      ASSETS: {
        fetch: async () => new Response("Not found", { status: 404 }),
      },
    },
    {
      waitUntil() {},
      passThroughOnException() {},
    },
  );
}

test("server-renders the lulang landing page", async () => {
  const response = await render();
  assert.equal(response.status, 200);
  assert.match(response.headers.get("content-type") ?? "", /^text\/html\b/i);

  const html = await response.text();
  assert.match(html, /<title>lulang — a language for numerical computing<\/title>/i);
  assert.match(html, /A small language for numerical computing/);
  assert.match(html, /ONLINE INTERPRETER/);
  assert.match(html, /lulang source editor/);
  assert.doesNotMatch(html, /codex-preview|react-loading-skeleton/i);
});

test("server-renders the source-linked benchmark observatory", async () => {
  const response = await render("/observatory");
  assert.equal(response.status, 200);
  const html = await response.text();
  assert.match(html, /Numbers without source are not results/);
  assert.match(html, /dot 2M × 20/);
  assert.match(html, /slerp 2M/);
  assert.match(html, /LU_LAYOUT/);
  assert.match(html, /benchmarks\/ir\/bench_slerp\.ll/);
  assert.match(html, /Missing runtimes are not estimated/);
  assert.match(html, /double slerp_checksum\(int64_t count\)/);
  assert.match(html, /6\.51×/);
  assert.match(html, /lulang_embedded\.ipynb/);
});
