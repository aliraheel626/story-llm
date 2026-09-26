import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "vite";

let server;
let entryDisplay;
const entry = { id: "old-reply", content: "Original reply" };

before(async () => {
  server = await createServer({ server: { middlewareMode: true }, appType: "custom", logLevel: "silent" });
  ({ entryDisplay } = await server.ssrLoadModule("/src/features/ledger/replacement.ts"));
});
after(async () => { await server?.close(); });

test("no stream shows the original reply and extras", () => {
  assert.deepEqual(entryDisplay(entry, null), { text: "Original reply", streaming: false, showOldExtras: true });
});

test("targeted replacement shows partial text and hides old extras", () => {
  assert.deepEqual(entryDisplay(entry, { mode: "replace", targetEntryId: entry.id, text: "Partial" }), {
    text: "Partial", streaming: true, showOldExtras: false,
  });
});

test("completed replacement text no longer shows a cursor", () => {
  assert.deepEqual(entryDisplay(entry, { mode: "replace", targetEntryId: entry.id, text: "New reply", textComplete: true }), {
    text: "New reply", streaming: false, showOldExtras: false,
  });
});

test("replacement targeting another entry leaves this one unchanged", () => {
  assert.deepEqual(entryDisplay(entry, { mode: "replace", targetEntryId: "other", text: "Partial" }), {
    text: "Original reply", streaming: false, showOldExtras: true,
  });
});

test("clearing a failed replacement restores original text and extras", () => {
  assert.equal(entryDisplay(entry, { mode: "replace", targetEntryId: entry.id, text: "Partial" }).showOldExtras, false);
  assert.deepEqual(entryDisplay(entry, null), { text: "Original reply", streaming: false, showOldExtras: true });
});
