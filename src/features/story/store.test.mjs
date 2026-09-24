import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "vite";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const calls = [];
const stories = new Map();
const hiddenEntries = new Map();
const images = new Map();
const entries = new Map();
let failTools = false;
let nextToolsSave;
let nextImageLoad;
let nextRetry;
let server;
let store;
let RollDisclosure;
let LedgerEntryView;
let rollFromEntry;
let groupRollsByEntry;
let toolCallsFromEvents;

const rollEvent = (id, target_entry_id, payload) => ({
  id, story_id: "story", seq: 1, kind: "diceroll", visibility: "hidden",
  content: null, target_entry_id, turn_id: null, created_at: "2026-09-23T12:00:00Z", payload,
});

const renderedReply = (storyId) => {
  const bundle = store.getState().bundles[storyId];
  const entry = bundle.entries.find((item) => item.kind === "narration");
  return renderToStaticMarkup(createElement(LedgerEntryView, {
    entry, storyId, isLast: true, retryEntryId: entry.id,
    rolls: groupRollsByEntry(bundle.hidden)[entry.id],
  }));
};

globalThis.__storyTestInvoke = async (command, args = {}) => {
  calls.push({ command, args });
  const story = stories.get(args.storyId);
  switch (command) {
    case "create_story": {
      const created = { id: `story-${stories.size + 1}`, title: "New story", settings_json: JSON.stringify(args.settings) };
      stories.set(created.id, { ...args.settings });
      return created;
    }
    case "get_story_narrator_tools": return { ...story.narrator_tools };
    case "save_story_narrator_tools":
      if (nextToolsSave) {
        const wait = nextToolsSave;
        nextToolsSave = null;
        await wait;
      }
      if (failTools) throw new Error("save rejected");
      story.narrator_tools = { ...args.tools };
      return;
    case "get_story_reasoning_effort": return story.reasoning_effort ?? "";
    case "save_story_reasoning_effort": story.reasoning_effort = args.reasoningEffort; return;
    case "list_ledger_entries": return {
      visible: entries.get(args.storyId) ?? [], hidden: hiddenEntries.get(args.storyId) ?? [],
      turns: (entries.get(args.storyId) ?? []).filter((entry) => entry.turn_id).map((entry) => ({ id: entry.turn_id, status: entry.turn_status ?? "complete" })),
    };
    case "list_images_for_story":
      if (nextImageLoad) {
        const load = nextImageLoad;
        nextImageLoad = null;
        return load;
      }
      return images.get(args.storyId) ?? [];
    case "list_entities": return [];
    case "retry_narration":
      if (nextRetry) {
        const wait = nextRetry;
        nextRetry = null;
        await wait;
      }
      return { entry_id: args.entryId, stream_id: `stream-${calls.length}` };
    case "erase_last_exchange": images.set(args.storyId, []); return ["see-action"];
    case "submit_turn": return {
      entry: { id: `action-${calls.length}`, story_id: args.storyId, kind: "player_message", payload: { input_mode: args.mode }, content: args.content, turn_id: `turn-${calls.length}` },
      stream_id: `stream-${calls.length}`,
    };
    default: throw new Error(`Unmocked command: ${command}`);
  }
};
globalThis.window = { __TAURI_INTERNALS__: { invoke: globalThis.__storyTestInvoke } };

before(async () => {
  server = await createServer({ configFile: false, optimizeDeps: { noDiscovery: true }, server: { middlewareMode: true } });
  ({ useStoryStore: store } = await server.ssrLoadModule("/src/features/story/store.ts"));
  ({ RollDisclosure } = await server.ssrLoadModule("/src/features/ledger/RollDisclosure.tsx"));
  ({ LedgerEntryView } = await server.ssrLoadModule("/src/features/ledger/LedgerEntryView.tsx"));
  ({ toolCallsFromEvents } = await server.ssrLoadModule("/src/features/ledger/TurnActivity.tsx"));
  ({ rollFromEntry, groupRollsByEntry } = await server.ssrLoadModule("/src/shared/types.ts"));
});

after(async () => { await server?.close(); });

test("draft defaults are all on and first create persists selected tools and effort", async () => {
  store.getState().startDraft();
  assert.ok(Object.values(store.getState().draftNarratorTools).every(Boolean));
  await store.getState().saveNarratorTools(null, { roll_check: false });
  await store.getState().saveReasoningEffort(null, "high");
  const story = await store.getState().createStory();
  const saved = stories.get(story.id);
  assert.equal(saved.narrator_tools.roll_check, false);
  assert.equal(saved.narrator_tools.illustrate_scene, true);
  assert.equal(saved.reasoning_effort, "high");
});

test("rapid independent toggles remain isolated by story and failed saves roll back", async () => {
  const first = store.getState().activeStoryId;
  store.getState().startDraft();
  const second = (await store.getState().createStory()).id;
  assert.equal(stories.get(second).reasoning_effort, undefined);
  await Promise.all([
    store.getState().saveNarratorTools(first, { get_entities: false }),
    store.getState().saveNarratorTools(first, { illustrate_scene: false }),
    store.getState().saveNarratorTools(second, { roll_check: false }),
  ]);
  assert.equal(stories.get(first).narrator_tools.get_entities, false);
  assert.equal(stories.get(first).narrator_tools.illustrate_scene, false);
  assert.equal(stories.get(first).narrator_tools.roll_check, false);
  assert.equal(stories.get(second).narrator_tools.roll_check, false);
  assert.equal(stories.get(second).narrator_tools.illustrate_scene, true);
  failTools = true;
  await assert.rejects(store.getState().saveNarratorTools(second, { create_entity: false }), /save rejected/);
  failTools = false;
  assert.equal(store.getState().bundles[second].narratorTools.create_entity, true);
});

test("replacement keeps original through streaming and failure, then clears old caches on success", async () => {
  const storyId = store.getState().activeStoryId;
  const original = { id: "original", story_id: storyId, kind: "narration", payload: { input_mode: "generated" }, content: "Original", turn_id: "turn-original" };
  const replacement = { ...original, id: "replacement", content: "Replacement" };
  entries.set(storyId, [original]);
  hiddenEntries.set(storyId, [rollEvent("old-roll", original.id, { chance_percent: 50, roll: 90, needed: 50, outcome: "success", seed: 1, reason: "Old roll" })]);
  images.set(storyId, [{ id: "old-image", entry_id: original.id, path: "old.png" }]);
  await Promise.all([store.getState().loadLedger(storyId), store.getState().loadImagesForStory(storyId)]);
  await store.getState().retryNarration(storyId, original.id);
  let stream = store.getState().bundles[storyId].streaming.streamId;
  store.getState()._appendDelta(stream, "Partial replacement");
  assert.equal(store.getState().bundles[storyId].entries[0].content, "Original");
  store.getState()._fail(stream, "provider failed");
  assert.equal(store.getState().bundles[storyId].entries[0].id, original.id);
  assert.equal(groupRollsByEntry(store.getState().bundles[storyId].hidden)[original.id][0].id, "old-roll");
  assert.match(renderedReply(storyId), /Old roll/);
  assert.equal(store.getState().bundles[storyId].imagesByEntry[original.id][0].id, "old-image");

  await store.getState().retryNarration(storyId, original.id);
  stream = store.getState().bundles[storyId].streaming.streamId;
  entries.set(storyId, [replacement]);
  hiddenEntries.set(storyId, [rollEvent("new-roll", replacement.id, { chance_percent: 40, roll: 72, needed: 60, outcome: "success", seed: 2, reason: "New roll" })]);
  images.set(storyId, []);
  store.getState()._finalize({ stream_id: stream, entry: replacement });
  assert.equal(store.getState().bundles[storyId].entries[0].id, replacement.id);
  assert.equal(store.getState().bundles[storyId].imagesByEntry[original.id], undefined);
  assert.equal(groupRollsByEntry(store.getState().bundles[storyId].hidden)[original.id], undefined);
  assert.doesNotMatch(renderedReply(storyId), /Old roll/);
  await store.getState().loadLedger(storyId);
  assert.equal(groupRollsByEntry(store.getState().bundles[storyId].hidden)[replacement.id][0].id, "new-roll");
  const html = renderedReply(storyId);
  assert.match(html, /New roll/);
  assert.doesNotMatch(html, /Old roll/);
  assert.equal(calls.some(({ command }) => command.startsWith("list_") && command.includes("roll")), false);
  assert.ok(calls.some(({ command }) => command === "list_entities"));
});

test("failed last turns render a status beside Retry", async () => {
  const storyId = store.getState().activeStoryId;
  const failed = { id: "failed-action", story_id: storyId, kind: "player_message", payload: { input_mode: "do" }, content: "Try", turn_id: "failed-turn", turn_status: "failed" };
  entries.set(storyId, [failed]);
  await store.getState().loadLedger(storyId);
  assert.deepEqual(store.getState().bundles[storyId].turns, [{ id: "failed-turn", status: "failed" }]);
  const html = renderToStaticMarkup(createElement(LedgerEntryView, {
    entry: failed, storyId, isLast: true, retryEntryId: failed.id, turnFailed: true,
  }));
  assert.match(html, /Retry/);
  assert.match(html, /Failed/);
  entries.set(storyId, [{ id: "replacement", story_id: storyId, kind: "narration", payload: { input_mode: "generated" }, content: "Replacement", turn_id: "turn-original" }]);
  await store.getState().loadLedger(storyId);
});

test("image events arriving during refresh survive an older list response", async () => {
  const storyId = store.getState().activeStoryId;
  let resolveLoad;
  nextImageLoad = new Promise((resolve) => { resolveLoad = resolve; });
  const refreshing = store.getState().loadImagesForStory(storyId);
  await Promise.resolve();
  store.getState()._imageGenerated({ id: "fresh-image", entry_id: "replacement", path: "new.png" });
  resolveLoad([]);
  await refreshing;
  assert.equal(store.getState().bundles[storyId].imagesByEntry.replacement[0].id, "fresh-image");
});

test("a new turn waits for in-flight tool saves", async () => {
  const storyId = store.getState().activeStoryId;
  let releaseSave;
  nextToolsSave = new Promise((resolve) => { releaseSave = resolve; });
  const save = store.getState().saveNarratorTools(storyId, { roll_check: true });
  const submitted = store.getState().submitTurn(storyId, "do", "Try the door");
  await Promise.resolve();
  assert.equal(calls.filter(({ command }) => command === "submit_turn").length, 0);
  releaseSave();
  await Promise.all([save, submitted]);
  assert.equal(stories.get(storyId).narrator_tools.roll_check, true);
  assert.equal(calls.filter(({ command }) => command === "submit_turn").length, 1);
  store.getState()._fail(store.getState().bundles[storyId].streaming.streamId, "test cleanup");
});

test("retry preparation blocks another action before streaming begins", async () => {
  const storyId = store.getState().activeStoryId;
  let releaseRetry;
  nextRetry = new Promise((resolve) => { releaseRetry = resolve; });
  const retry = store.getState().retryNarration(storyId, "replacement");
  assert.equal(store.getState().bundles[storyId].requestPending, true);
  await assert.rejects(store.getState().submitTurn(storyId, "do", "Competing action"), /already in progress/);
  releaseRetry();
  await retry;
  assert.equal(store.getState().bundles[storyId].requestPending, false);
  store.getState()._fail(store.getState().bundles[storyId].streaming.streamId, "test cleanup");
});

test("erasing a trailing See refreshes images attached to prior narration", async () => {
  const storyId = store.getState().activeStoryId;
  assert.equal(store.getState().bundles[storyId].imagesByEntry.replacement[0].id, "fresh-image");
  await store.getState().eraseLastExchange(storyId);
  assert.equal(store.getState().bundles[storyId].imagesByEntry.replacement, undefined);
});

test("snapshot roll selector preserves both factor snapshots and optional fields", () => {
  const factors = [
    { entity_id: "player", entity_name: "You", attribute_id: "stealth", attribute_name: "Stealth", value: 8, min: 0, max: 10 },
    { entity_id: "guard", entity_name: "Guard", attribute_id: "perception", attribute_name: "Perception", value: 6, min: 0, max: 10 },
  ];
  const payload = { chance_percent: 60, roll: 70, needed: 40, outcome: "success", reason: "Sneak", chance_source: "attributes", factors, seed: 42 };
  const event = rollEvent("factored", "narration", payload);
  const grouped = groupRollsByEntry([rollEvent("earlier", "other", { chance_percent: 50, roll: 50, needed: 50, outcome: "success", seed: 1 }), event]);
  assert.deepEqual(grouped.narration, [{
    id: "factored", entry_id: "narration", created_at: event.created_at,
    reason: "Sneak", chance_percent: 60, roll: 70, needed: 40, outcome: "success", seed: 42,
    chance_source: "attributes", factors,
  }]);
  assert.equal(grouped.other[0].chance_source, null);
  assert.equal(grouped.other[0].reason, null);
  assert.deepEqual(grouped.other[0].factors, []);
  assert.equal(rollFromEntry(rollEvent("legacy", "narration", { ...payload, chance_source: undefined, reason: false, factors: undefined })).chance_source, null);
});

test("snapshot selector validates roll shapes without interpreting outcomes", () => {
  const base = { chance_percent: 50, roll: 50, needed: 50, outcome: "success", seed: 42 };
  const factor = { entity_id: "p", entity_name: "You", attribute_id: "a", attribute_name: "Agility", value: 5, min: 0, max: 10 };
  const invalid = [
    { chance_percent: 50.5 }, { roll: 1.5 }, { needed: 49.5 }, { outcome: "" },
    { seed: "42" }, { seed: 1.5 },
    { factors: null }, { factors: {} }, { factors: [factor, factor, factor] },
    { factors: [{ ...factor, entity_name: null }] }, { factors: [{ ...factor, min: 10 }] },
    { factors: [{ ...factor, value: 11 }] }, { factors: [{ ...factor, value: Infinity }] },
    { chance_source: "unknown" }, { chance_source: 7 },
  ];
  const events = invalid.map((patch, index) => rollEvent(`invalid-${index}`, "narration", { ...base, ...patch }));
  events.push(rollEvent("untargeted", null, base));
  events.push({ ...rollEvent("not-a-roll", "narration", base), kind: "entity_queried" });
  events.push(rollEvent("null-payload", "narration", null));
  events.push(rollEvent("array-payload", "narration", []));
  assert.deepEqual(Object.keys(groupRollsByEntry(events)), []);
  assert.equal(rollFromEntry(rollEvent("good", "narration", { ...base, chance_source: "default" })).chance_source, "default");
  const permissive = rollFromEntry(rollEvent("permissive", "narration", {
    ...base, chance_percent: 101, roll: -1, needed: 900, outcome: "critical", chance_source: "attributes",
  }));
  assert.equal(permissive.outcome, "critical");
  assert.equal(permissive.needed, 900);
  assert.equal(rollFromEntry(rollEvent("disagreement", "narration", {
    ...base, roll: 99, outcome: "failure",
  })).outcome, "failure");
});

test("snapshot roll grouping safely handles inherited-property target IDs", () => {
  const payload = { chance_percent: 50, roll: 50, needed: 50, outcome: "success", seed: 42 };
  const grouped = groupRollsByEntry([
    rollEvent("prototype-roll", "__proto__", payload),
    rollEvent("constructor-roll", "constructor", payload),
  ]);
  assert.equal(Object.getPrototypeOf(grouped), null);
  assert.equal(grouped.__proto__[0].id, "prototype-roll");
  assert.equal(grouped.constructor[0].id, "constructor-roll");
});

test("turn activity prefers captured tool calls and preserves false outcomes", () => {
  const base = {
    story_id: "story", visibility: "hidden", target_entry_id: "narration",
    turn_id: "turn", created_at: "2026-09-23T12:00:00Z",
  };
  const hidden = [
    { ...base, id: "effect", seq: 1, kind: "entity_updated", content: "Legacy effect", payload: {} },
    { ...base, id: "failed", seq: 3, kind: "tool_call", content: "Checking who's here…", payload: { tool: "get_entities", args: {}, result: "failed", ok: false } },
    { ...base, id: "noop", seq: 2, kind: "tool_call", content: "Adjusting Trust…", payload: { tool: "adjust_entity_attribute", args: {}, result: { applied: false }, ok: true } },
  ];

  assert.deepEqual(toolCallsFromEvents("narration", hidden), [
    { key: "noop", label: "Adjusting Trust…", done: true, ok: true },
    { key: "failed", label: "Checking who's here…", done: true, ok: false },
  ]);
  assert.deepEqual(toolCallsFromEvents("narration", [hidden[0]]), [
    { key: "effect", label: "Legacy effect", done: true, ok: true },
  ]);
  const roll = rollEvent("roll", "narration", {
    chance_percent: 50, roll: 12, needed: 50, outcome: "complication", seed: 7, reason: "opening the vault",
  });
  roll.content = "Resultado de dados heredado";
  assert.deepEqual(toolCallsFromEvents("narration", [roll]), [
    { key: "roll", label: "Roll for opening the vault: complication", done: true, ok: true },
  ]);
});

test("default chance rolls show their source and no factors alongside threshold, draw, and seed", () => {
  const roll = { id: "r", entry_id: "replacement", reason: "Leap across the gap", chance_percent: 50, chance_source: "default", factors: [], roll: 72, needed: 47, outcome: "success", seed: 42 };
  const html = renderToStaticMarkup(createElement(RollDisclosure, { roll }));
  assert.match(html, /Leap across the gap/);
  assert.match(html, /Chance source: Default/);
  assert.match(html, /No attribute factors/);
  assert.match(html, /needed 47\+/);
  assert.match(html, /rolled 72/);
  assert.match(html, /Seed: 42/);
  assert.match(html, /success/);
  assert.doesNotMatch(html, /<li>/);
});

test("attribute chance rolls show both factor snapshot names, values, and ranges", () => {
  const roll = {
    id: "r2", entry_id: "replacement", reason: "Outrun the guard", chance_percent: 65,
    chance_source: "attributes", roll: 80, needed: 31, outcome: "success", seed: 123,
    factors: [
      { entity_id: "hero", entity_name: "Mira", attribute_id: "speed", attribute_name: "Speed", value: 8, min: 0, max: 10 },
      { entity_id: "guard", entity_name: "Guard", attribute_id: "awareness", attribute_name: "Awareness", value: 3, min: 1, max: 12 },
    ],
  };
  const html = renderToStaticMarkup(createElement(RollDisclosure, { roll }));
  assert.match(html, /Chance source: Attributes/);
  assert.match(html, /Mira: Speed 8 \(range 0-10\)/);
  assert.match(html, /Guard: Awareness 3 \(range 1-12\)/);
  assert.match(html, /needed 31\+/);
  assert.match(html, /rolled 80/);
  assert.match(html, /Seed: 123/);
});

test("chance-only legacy rolls do not claim a default or attribute source", () => {
  const roll = { id: "old", entry_id: "replacement", reason: null, chance_percent: 40, roll: 72, needed: 60, outcome: "success", seed: 9 };
  const html = renderToStaticMarkup(createElement(RollDisclosure, { roll }));
  assert.match(html, /Chance source: Unspecified/);
});
