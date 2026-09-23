import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer } from "vite";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const calls = [];
const stories = new Map();
const rolls = new Map();
const images = new Map();
const entries = new Map();
let failTools = false;
let nextToolsSave;
let nextImageLoad;
let nextRetry;
let server;
let store;
let RollDisclosure;

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
    case "list_ledger_entries": return { visible: entries.get(args.storyId) ?? [], hidden: [] };
    case "list_images_for_story":
      if (nextImageLoad) {
        const load = nextImageLoad;
        nextImageLoad = null;
        return load;
      }
      return images.get(args.storyId) ?? [];
    case "list_rolls_for_story": return rolls.get(args.storyId) ?? [];
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
      entry: { id: `action-${calls.length}`, story_id: args.storyId, kind: "player_message", payload: { input_mode: args.mode }, content: args.content },
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
  const original = { id: "original", story_id: storyId, kind: "narration", payload: { input_mode: "generated" }, content: "Original" };
  const replacement = { ...original, id: "replacement", content: "Replacement" };
  entries.set(storyId, [original]);
  rolls.set(storyId, [{ id: "old-roll", entry_id: original.id, chance_percent: 50, roll: 90, outcome: "success", seed: 1 }]);
  images.set(storyId, [{ id: "old-image", entry_id: original.id, path: "old.png" }]);
  await Promise.all([store.getState().loadLedger(storyId), store.getState().loadRollsForStory(storyId), store.getState().loadImagesForStory(storyId)]);
  await store.getState().retryNarration(storyId, original.id);
  let stream = store.getState().bundles[storyId].streaming.streamId;
  store.getState()._appendDelta(stream, "Partial replacement");
  assert.equal(store.getState().bundles[storyId].entries[0].content, "Original");
  store.getState()._fail(stream, "provider failed");
  assert.equal(store.getState().bundles[storyId].entries[0].id, original.id);
  assert.equal(store.getState().bundles[storyId].rollsByEntry[original.id][0].id, "old-roll");
  assert.equal(store.getState().bundles[storyId].imagesByEntry[original.id][0].id, "old-image");

  await store.getState().retryNarration(storyId, original.id);
  stream = store.getState().bundles[storyId].streaming.streamId;
  entries.set(storyId, [replacement]);
  rolls.set(storyId, [{ id: "new-roll", entry_id: replacement.id, chance_percent: 40, roll: 72, outcome: "success", seed: 2 }]);
  images.set(storyId, []);
  store.getState()._finalize({ stream_id: stream, entry: replacement });
  assert.equal(store.getState().bundles[storyId].entries[0].id, replacement.id);
  assert.equal(store.getState().bundles[storyId].imagesByEntry[original.id], undefined);
  assert.equal(store.getState().bundles[storyId].rollsByEntry[original.id], undefined);
  await Promise.all([store.getState().loadLedger(storyId), store.getState().loadRollsForStory(storyId)]);
  assert.equal(store.getState().bundles[storyId].rollsByEntry[replacement.id][0].id, "new-roll");
  assert.ok(calls.some(({ command }) => command === "list_entities"));
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

test("default chance rolls show their source and no factors alongside threshold, draw, and seed", () => {
  const roll = { id: "r", entry_id: "replacement", reason: "Leap across the gap", chance_percent: 50, chance_source: "default", factors: [], roll: 72, outcome: "success", seed: 42 };
  const html = renderToStaticMarkup(createElement(RollDisclosure, { roll }));
  assert.match(html, /Leap across the gap/);
  assert.match(html, /Chance source: Default/);
  assert.match(html, /No attribute factors/);
  assert.match(html, /needed 50\+/);
  assert.match(html, /rolled 72/);
  assert.match(html, /Seed: 42/);
  assert.match(html, /success/);
  assert.doesNotMatch(html, /<li>/);
});

test("attribute chance rolls show both factor snapshot names, values, and ranges", () => {
  const roll = {
    id: "r2", entry_id: "replacement", reason: "Outrun the guard", chance_percent: 65,
    chance_source: "attributes", roll: 80, outcome: "success", seed: 123,
    factors: [
      { entity_id: "hero", entity_name: "Mira", attribute_id: "speed", attribute_name: "Speed", value: 8, min: 0, max: 10 },
      { entity_id: "guard", entity_name: "Guard", attribute_id: "awareness", attribute_name: "Awareness", value: 3, min: 1, max: 12 },
    ],
  };
  const html = renderToStaticMarkup(createElement(RollDisclosure, { roll }));
  assert.match(html, /Chance source: Attributes/);
  assert.match(html, /Mira: Speed 8 \(range 0-10\)/);
  assert.match(html, /Guard: Awareness 3 \(range 1-12\)/);
  assert.match(html, /needed 35\+/);
  assert.match(html, /rolled 80/);
  assert.match(html, /Seed: 123/);
});

test("chance-only legacy rolls do not claim a default or attribute source", () => {
  const roll = { id: "old", entry_id: "replacement", reason: null, chance_percent: 40, roll: 72, outcome: "success", seed: 9 };
  const html = renderToStaticMarkup(createElement(RollDisclosure, { roll }));
  assert.match(html, /Chance source: Unspecified/);
});
