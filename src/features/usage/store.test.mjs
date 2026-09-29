import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "vite";

const calls = [];
let nextResponse;
let server;
let store;
let formatUsd;
let formatTokens;
let formatSeconds;

const sampleBreakdown = {
  turns: [{ turn_id: "t1", total_cost_usd: 0.043 }],
  images: [{ asset_id: "a1", turn_id: "t1", cost_usd: 0.039, duration_ms: 14200 }],
};

globalThis.window = { __TAURI_INTERNALS__: { invoke: async (command, args) => {
  calls.push({ command, args });
  return nextResponse ? nextResponse(command, args) :
    command === "get_story_usage_breakdown" ? sampleBreakdown : { total_cost_usd: 3 };
} } };

before(async () => {
  server = await createServer({ configFile: false, optimizeDeps: { noDiscovery: true }, server: { middlewareMode: true } });
  ({ useUsageStore: store, formatUsd, formatTokens, formatSeconds } = await server.ssrLoadModule("/src/features/usage/store.ts"));
});

after(async () => { await server?.close(); });

test("load stores a story's usage and breakdown by turn and asset", async () => {
  await store.getState().load("A");
  assert.deepEqual(calls.slice(-2), [
    { command: "get_story_usage", args: { storyId: "A" } },
    { command: "get_story_usage_breakdown", args: { storyId: "A" } },
  ]);
  assert.equal(store.getState().byStory.A.total_cost_usd, 3);
  assert.equal(store.getState().breakdownByStory.A.turns.t1.total_cost_usd, 0.043);
  assert.equal(store.getState().breakdownByStory.A.images.a1.duration_ms, 14200);
});

test("a stale usage and breakdown response cannot overwrite a newer load", async () => {
  let resolveFirstUsage;
  let resolveFirstBreakdown;
  let requests = 0;
  nextResponse = (command) => {
    if (command === "get_story_usage") return ++requests === 1
      ? new Promise((resolve) => { resolveFirstUsage = resolve; })
      : Promise.resolve({ total_cost_usd: 2 });
    return requests === 1
      ? new Promise((resolve) => { resolveFirstBreakdown = resolve; })
      : Promise.resolve({ turns: [{ turn_id: "t2", total_cost_usd: 2 }], images: [] });
  };
  const first = store.getState().load("A");
  await store.getState().load("A");
  resolveFirstUsage({ total_cost_usd: 1 });
  resolveFirstBreakdown(sampleBreakdown);
  await first;
  assert.equal(store.getState().byStory.A.total_cost_usd, 2);
  assert.equal(store.getState().breakdownByStory.A.turns.t2.total_cost_usd, 2);
  assert.equal(store.getState().breakdownByStory.A.turns.t1, undefined);
  nextResponse = undefined;
});

test("formatters display cost precision and compact cache tokens", () => {
  assert.equal(formatUsd(0), "$0.00");
  assert.equal(formatUsd(0.0123), "$0.0123");
  assert.equal(formatUsd(1.25), "$1.25");
  assert.equal(formatTokens(999), "999");
  assert.equal(formatTokens(182400), "182.4K");
  assert.equal(formatSeconds(14200), "14.2 s");
  assert.equal(formatSeconds(null), "—");
});
