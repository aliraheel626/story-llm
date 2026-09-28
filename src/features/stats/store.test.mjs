import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "vite";

const calls = [];
let nextResponse;
let server;
let store;
let formatUsd;
let formatTokens;

globalThis.window = { __TAURI_INTERNALS__: { invoke: async (command, args) => {
  calls.push({ command, args });
  return nextResponse ? nextResponse() : { total_cost_usd: 3 };
} } };

before(async () => {
  server = await createServer({ configFile: false, optimizeDeps: { noDiscovery: true }, server: { middlewareMode: true } });
  ({ useStatsStore: store, formatUsd, formatTokens } = await server.ssrLoadModule("/src/features/stats/store.ts"));
});

after(async () => { await server?.close(); });

test("load stores a story's stats", async () => {
  await store.getState().load("A");
  assert.deepEqual(calls.at(-1), { command: "get_story_stats", args: { storyId: "A" } });
  assert.equal(store.getState().byStory.A.total_cost_usd, 3);
});

test("a stale stats response cannot overwrite a newer load", async () => {
  let resolveFirst;
  let requests = 0;
  nextResponse = () => ++requests === 1
    ? new Promise((resolve) => { resolveFirst = resolve; })
    : Promise.resolve({ total_cost_usd: 2 });
  const first = store.getState().load("A");
  await store.getState().load("A");
  resolveFirst({ total_cost_usd: 1 });
  await first;
  assert.equal(store.getState().byStory.A.total_cost_usd, 2);
  nextResponse = undefined;
});

test("formatters display cost precision and compact cache tokens", () => {
  assert.equal(formatUsd(0), "$0.00");
  assert.equal(formatUsd(0.0123), "$0.0123");
  assert.equal(formatUsd(1.25), "$1.25");
  assert.equal(formatTokens(999), "999");
  assert.equal(formatTokens(182400), "182.4K");
});
