// Ignored evidence: reproduce the prepare-only probe and check exact live application.
// Store actions/hooks are mocked; this does not verify the real store queue or browser focus.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { renderToStaticMarkup } from "react-dom/server";
import ts from "typescript";

const require = createRequire(import.meta.url);
const root = fileURLToPath(new URL("../../../", import.meta.url));
const panelPath = path.join(root, "src/features/characters/CharactersPanel.tsx");
const patchPath = path.join(root, "docs/report/entities/c6-panel-prepared.patch");
const lines = ts.sys.readFile(patchPath).trimEnd().split(/\r?\n/);
assert.equal(lines[0], "*** Begin Patch");
assert.equal(lines.at(-1), "*** End Patch");
assert.deepEqual(lines.filter((line) => line.startsWith("*** Update File:")), [
  "*** Update File: src/features/characters/CharactersPanel.tsx",
]);
const source = lines.filter((line) => line.startsWith("+")).map((line) => line.slice(1)).join("\n") + "\n";
const before = lines.filter((line) => line.startsWith("-")).map((line) => line.slice(1)).join("\n");
assert.equal(before.trimEnd().split("\n").length, 244);
assert.equal(ts.sys.readFile(panelPath).replace(/\r\n/g, "\n"), source);
assert.ok(!lines.some((line) => line.startsWith("+") && /\s+$/.test(line)));
assert.deepEqual(source.match(/charactersApi\.[A-Za-z]+/g), ["charactersApi.listRegistry"]);
assert.ok(!/\benqueue\b/.test(source));
console.log("PASS: single owned patch, 244-line baseline, exact live/prepared parity, whitespace, registry-only direct API");
console.log(`Panel source: ${source.trimEnd().split("\n").length} lines; SHA256 ${createHash("sha256").update(source).digest("hex")}`);

const labels = {
  known_as: "Known as", appearance_anchor: "Appearance", gender: "Gender", age: "Age",
  role: "Role", location: "Location", outfit: "Outfit",
};
const key = (file) => path.resolve(root, file).toLowerCase();
const virtual = new Map([
  [key(panelPath), source],
  [key("src/shared/types.ts"), `
    export interface CharacterFields {
      ${Object.keys(labels).map((field) => `${field}:string|null;`).join("")}
    }
    export type CharacterPatch = Partial<CharacterFields>;
    export interface EntityLink {
      from_id:string; to_id:string; label:string; direction:"one_way"|"both"; description:string|null;
    }
    export interface Entity extends CharacterFields {
      id:string; story_id:string; kind:"character"|"relationship"; name:string; created_at:string; link:EntityLink|null;
    }
    export interface EntityAttributeValue {
      story_id:string; entity_id:string; attribute_id:string; canonical_name:string; value:number;
      min:number; max:number; updated_at:string; source:string;
    }
    export interface AttributeRegistryEntry {
      id:string; canonical_name:string; aliases_json:string; entity_kinds_json:string;
      min:number; max:number; category:string; is_user_created:boolean;
      created_in_story_id:string|null; created_at:string;
    }
    export declare const CHARACTER_FIELD_LABELS:Record<keyof CharacterFields,string>;
  `],
  [key("src/features/story/store.ts"), `
    import type {Entity,EntityAttributeValue,CharacterPatch} from "../../shared/types";
    interface State {
      activeStoryId:string|null;
      bundles:Record<string,{entities:Entity[];entitiesLoading:boolean;attributesByEntity:Record<string,EntityAttributeValue[]>}>;
      loadEntities(s:string):Promise<void>;
      createEntity(s:string,name:string,fields:CharacterPatch):Promise<void>;
      updateEntity(s:string,id:string,name:string|undefined,fields:CharacterPatch,drafts?:Record<string,string>):Promise<void>;
      deleteEntity(s:string,id:string):Promise<void>;
      setEntityAttribute(s:string,id:string,attr:string,value:number):Promise<EntityAttributeValue>;
      removeEntityAttribute(s:string,id:string,attr:string):Promise<void>;
    }
    export declare function useStoryStore<T>(selector:(state:State)=>T):T;
  `],
  [key("src/features/characters/api.ts"), `
    import type {AttributeRegistryEntry} from "../../shared/types";
    export declare const charactersApi:{listRegistry():Promise<AttributeRegistryEntry[]>};
  `],
]);
const config = ts.readConfigFile(path.join(root, "tsconfig.json"), ts.sys.readFile);
assert.equal(config.error, undefined);
const options = ts.convertCompilerOptionsFromJson(config.config.compilerOptions, root).options;
const host = ts.createCompilerHost(options);
const getSource = host.getSourceFile.bind(host);
const read = host.readFile.bind(host);
host.getSourceFile = (file, version, ...rest) => virtual.has(key(file))
  ? ts.createSourceFile(file, virtual.get(key(file)), version, true)
  : getSource(file, version, ...rest);
host.readFile = (file) => virtual.get(key(file)) ?? read(file);
host.writeFile = () => { throw new Error("Unexpected compiler write"); };
const diagnostics = ts.getPreEmitDiagnostics(ts.createProgram([panelPath], options, host));
if (diagnostics.length) {
  console.error(ts.formatDiagnosticsWithColorAndContext(diagnostics, {
    getCanonicalFileName: (file) => file, getCurrentDirectory: () => root, getNewLine: () => "\n",
  }));
  throw new Error("Isolated C6 protocol typecheck failed");
}
console.log("PASS: strict in-memory TypeScript check against agreed C6 interfaces and actual React typings");

const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX },
  reportDiagnostics: true,
});
assert.equal(compiled.diagnostics.length, 0);
const fields = Object.fromEntries(Object.keys(labels).map((field) => [field, null]));
const character = (id, name, extra = {}) => ({
  ...fields, id, name, story_id: "s1", kind: "character", created_at: "now", link: null, ...extra,
});
const you = character("you", "You");
const mira = character("mira", "Mira", {
  gender: "female", age: "34", role: "smuggler", location: "Rusty Anchor", outfit: "grey cloak", appearance_anchor: "black hair",
});
const stranger = character("stranger", "SeleneSecret", { known_as: "the hooded stranger" });
const relation = {
  ...fields, id: "rel", story_id: "s1", kind: "relationship", name: "SeleneSecret to You", created_at: "now",
  link: { from_id: "stranger", to_id: "you", label: "watching", direction: "both", description: "A wary alliance" },
};
const directed = {
  ...relation, id: "directed", name: "Mira to You",
  link: { from_id: "mira", to_id: "you", label: "resentment", direction: "one_way", description: null },
};
const registry = [
  ["health", "Health", "character", 0, 10], ["nerve", "Nerve", "character", 0, 10],
  ["trust", "Trust", "relationship", -10, 10], ["affection", "Affection", "relationship", 0, 10],
  ["tension", "Tension", "relationship", -5, 5],
].map(([id, canonical_name, kind, min, max]) => ({ id, canonical_name, entity_kinds_json: JSON.stringify([kind]), min, max }));
const attr = (entity_id, attribute_id, value) => ({
  ...registry.find((item) => item.id === attribute_id),
  story_id: "s1", entity_id, attribute_id, value, updated_at: "now", source: "user",
});
const bundle = {
  entities: [you, mira, stranger, relation, directed], entitiesLoading: false,
  attributesByEntity: { mira: [attr("mira", "health", 4)], rel: [attr("rel", "affection", 8), attr("rel", "trust", -2)] },
};
const calls = [];
const errors = [];
let updatePending = null, deletePending = null, setPending = null, removePending = null, rejectAdd = false;
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
const store = {
  activeStoryId: "s1", bundles: { s1: bundle, s2: { entities: [], entitiesLoading: false, attributesByEntity: {} } },
  loadEntities: async (story) => { calls.push(["load", story]); },
  createEntity: async (...args) => { calls.push(["create", ...args]); },
  updateEntity: (...args) => { calls.push(["update", ...args]); return updatePending?.promise ?? Promise.resolve(); },
  deleteEntity: (...args) => { calls.push(["delete", ...args]); return deletePending?.promise ?? Promise.resolve(); },
  setEntityAttribute: (...args) => {
    calls.push(["set", ...args]);
    if (rejectAdd) return Promise.reject(new Error("Expected test rejection"));
    return setPending?.promise ?? Promise.resolve(attr(args[1], args[2], args[3]));
  },
  removeEntityAttribute: (...args) => { calls.push(["remove", ...args]); return removePending?.promise ?? Promise.resolve(); },
};
let cursor = 0, tree, effects = [];
const states = [], deps = new Map(), storage = new Map();
const hooks = {
  useState(initial) {
    const i = cursor++;
    if (!(i in states)) states[i] = typeof initial === "function" ? initial() : initial;
    return [states[i], (next) => { states[i] = typeof next === "function" ? next(states[i]) : next; }];
  },
  useEffect(callback, next) {
    const i = cursor++, prev = deps.get(i);
    if (!prev || next.some((value, j) => !Object.is(value, prev[j]))) { deps.set(i, next); effects.push(callback); }
  },
};
const module = { exports: {} };
const sandbox = {
  exports: module.exports, console: { error: (error) => errors.push(error) },
  window: { localStorage: { getItem: (item) => storage.get(item) ?? null, setItem: (item, value) => storage.set(item, value) } },
  require: (id) => {
    if (id === "react") return hooks;
    if (id === "../../shared/types") return { CHARACTER_FIELD_LABELS: labels };
    if (id === "../story/store") return { useStoryStore: (selector) => selector(store) };
    if (id === "./api") return { charactersApi: { listRegistry: async () => registry } };
    return require(id);
  },
};
vm.runInNewContext(compiled.outputText, sandbox);
const render = () => { cursor = 0; effects = []; tree = module.exports.CharactersPanel(); return tree; };
const walk = (node) => Array.isArray(node) ? node.flatMap(walk)
  : node && typeof node === "object" && node.props ? [node, ...walk(node.props.children)] : [];
const find = (label, parent = tree) => {
  const node = walk(parent).find((item) => item.props["aria-label"] === label);
  assert.ok(node, `Missing ${label}`);
  return node;
};
const row = (id) => {
  const node = walk(tree).find((item) => item.props["data-entity-id"] === id);
  assert.ok(node, `Missing entity ${id}`);
  return node;
};
const change = (label, value, parent = tree) => find(label, parent).props.onChange({ target: { value } });
const click = (label, parent = tree) => find(label, parent).props.onClick();
const flush = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); };
const latest = (kind) => calls.filter((call) => call[0] === kind).at(-1);
const optionIds = (parent) => walk(find("Attribute to add", parent)).filter((node) => node.type === "option").map((node) => node.props.value);

render();
for (const effect of effects) effect();
await flush();
render();
let html = renderToStaticMarkup(tree);
assert.ok(!html.includes("SeleneSecret"));
assert.ok(html.includes("female") && html.includes("34") && html.includes("smuggler"));
assert.ok(html.includes("Rusty Anchor") && html.includes("grey cloak") && html.includes("black hair"));
assert.equal(find("Reveal true name", row("stranger")).props["aria-expanded"], false);
click("Reveal true name", row("stranger")); render();
assert.ok(renderToStaticMarkup(tree).includes("SeleneSecret"));
click("Hide true name", row("stranger")); render();
assert.ok(!renderToStaticMarkup(tree).includes("SeleneSecret"));
click("Relationships"); render();
html = renderToStaticMarkup(tree);
assert.ok(!html.includes("SeleneSecret"));
assert.ok(html.includes("the hooded stranger") && html.includes("Affection 8") && html.includes("Trust -2"));
assert.ok(html.includes("\u2194") && html.includes("\u2192"));
assert.equal(find("Relationships").props["aria-pressed"], true);
assert.ok(!walk(tree).some((node) => node.type === "button" && node.props["aria-label"] === "New character"));
assert.equal(storage.get("story-llm:entity-tab:s1"), "relationship");
click("Edit relationship", row("rel")); render();
assert.equal(find("Label", row("rel")).props.value, "watching");
assert.equal(find("Direction", row("rel")).props.readOnly, true);
assert.equal(find("Description", row("rel")).props.readOnly, true);
assert.ok(optionIds(row("rel")).includes("tension") && !optionIds(row("rel")).includes("health"));
change("Label", "wary", row("rel")); render();
await click("Save relationship", row("rel")); render();
assert.equal(latest("update")[2], "rel");
assert.equal(latest("update")[3], "wary");
assert.equal(Object.keys(latest("update")[4]).length, 0);
assert.equal(Object.keys(latest("update")[5]).length, 0);
console.log("PASS: actual React static render hides unrevealed true names; aliases, both arrows, relationship stats, label editor, readonly direction/description");

click("Characters"); render();
click("New character"); render();
for (const label of ["Name", ...Object.values(labels)]) find(label);
change("Name", "Old Tom"); change("Role", "innkeeper"); change("Location", "behind the bar"); change("Outfit", "stained apron"); render();
await click("Create character"); render();
assert.equal(latest("create")[2], "Old Tom");
assert.equal(latest("create")[3].role, "innkeeper");
assert.equal(latest("create")[3].location, "behind the bar");
assert.equal(latest("create")[3].outfit, "stained apron");
assert.equal(Object.keys(latest("create")[3]).length, 3);
click("Edit character", row("mira")); render();
assert.ok(optionIds(row("mira")).includes("nerve") && !optionIds(row("mira")).includes("tension"));
change("Age", "35", row("mira"));
bundle.entities = bundle.entities.map((entity) => entity.id === "mira"
  ? { ...entity, name: "Mira Current", role: "captain", appearance_anchor: "later narration facts" } : entity);
render();
assert.equal(find("Role", row("mira")).props.value, "smuggler");
await click("Save character", row("mira")); render();
assert.equal(latest("update")[3], undefined);
assert.equal(latest("update")[4].age, "35");
assert.equal(Object.keys(latest("update")[4]).length, 1);
click("Edit character", row("mira")); render();
change("Gender", "", row("mira")); render();
await click("Save character", row("mira")); render();
assert.equal(latest("update")[4].gender, null);
assert.equal(Object.keys(latest("update")[4]).length, 1);
click("Edit character", row("mira")); render();
change("Name", "Mira Renamed", row("mira")); render();
await click("Save character", row("mira")); render();
assert.equal(latest("update")[3], "Mira Renamed");
console.log("PASS: eight form labels, own-kind registry, create fields, original-baseline changed-only Save, literal null clear and changed-only name");

updatePending = deferred();
click("Edit character", row("mira")); render();
change("Age", "36", row("mira")); render();
const saving = click("Save character", row("mira")); render();
assert.equal(find("Name", row("mira")).props.disabled, true);
click("Cancel character edit", row("mira")); render();
click("Edit character", row("stranger")); render();
updatePending.resolve(); await saving; updatePending = null; render();
assert.equal(find("Name", row("stranger")).props.value, "SeleneSecret");
click("Cancel character edit", row("stranger")); render();
deletePending = deferred();
click("Edit character", row("mira")); render();
const deleting = click("Delete character", row("mira")); render();
click("Cancel character edit", row("mira")); render();
click("Edit character", row("mira")); render();
deletePending.resolve(); await deleting; deletePending = null; render();
find("Save character", row("mira"));
console.log("PASS: pending Save/Delete cannot close a new edit session, including same-entity reopen");

setPending = deferred();
change("Health", "7", row("mira")); render();
find("Health", row("mira")).props.onBlur();
assert.equal(latest("set")[4], 7);
change("Health", "8", row("mira")); render();
change("Health", "7", row("mira")); render();
bundle.attributesByEntity.mira = [attr("mira", "health", 9)];
setPending.resolve(attr("mira", "health", 7)); await flush(); setPending = null; render();
assert.equal(find("Health", row("mira")).props.value, "7");
await click("Save character", row("mira")); render();
assert.equal(latest("update")[5].health, "7");
click("Edit character", row("mira")); render();
setPending = deferred(); removePending = deferred();
change("Health", "6", row("mira")); render();
find("Health", row("mira")).props.onBlur();
click("Remove Health", row("mira")); render();
assert.ok(!walk(row("mira")).some((node) => node.props["aria-label"] === "Health"));
await click("Save character", row("mira")); render();
assert.equal(Object.keys(latest("update")[5]).length, 0);
setPending.resolve(attr("mira", "health", 6)); removePending.resolve(); await flush();
setPending = null; removePending = null;
console.log("PASS: pending stat result preserves later same-value retyping; immediate removal excludes dirty draft from Save");

click("Edit character", row("mira")); render();
change("Attribute to add", "nerve", row("mira")); render();
await click("Add attribute", row("mira")); render();
assert.equal(latest("set")[3], "nerve"); assert.equal(latest("set")[4], 5);
const setCount = calls.filter((call) => call[0] === "set").length;
change("Health", "", row("mira")); render();
find("Health", row("mira")).props.onBlur();
assert.equal(calls.filter((call) => call[0] === "set").length, setCount);
await click("Save character", row("mira")); render();
assert.equal(Object.keys(latest("update")[5]).length, 0);
click("Edit character", row("mira")); render();
rejectAdd = true;
change("Attribute to add", "nerve", row("mira")); render();
click("Add attribute", row("mira")); await flush(); render();
assert.equal(errors.length, 1);
assert.equal(find("Attribute to add", row("mira")).props.value, "nerve");
assert.ok(optionIds(row("mira")).includes("nerve"));
rejectAdd = false;
await click("Add attribute", row("mira")); render();
assert.equal(latest("set")[3], "nerve");
click("Cancel character edit", row("mira")); render();
console.log("PASS: Add uses queued-store setter midpoint; blank stats never write zero; failed Add selection remains retryable");

click("Relationships"); render();
store.activeStoryId = "s2"; render();
for (const effect of effects) effect(); await flush(); render();
assert.equal(find("Characters").props["aria-pressed"], true);
assert.equal(storage.get("story-llm:entity-tab:s1"), "relationship");
assert.equal(storage.has("story-llm:entity-tab:s2"), false);
store.activeStoryId = "s1"; render();
for (const effect of effects) effect(); await flush(); render();
assert.equal(find("Relationships").props["aria-pressed"], true);
click("Characters"); render();
assert.equal(find("Reveal true name", row("stranger")).props["aria-expanded"], false);
sandbox.window.localStorage.getItem = () => { throw new Error("Unavailable storage"); };
sandbox.window.localStorage.setItem = () => { throw new Error("Unavailable storage"); };
click("Relationships"); render();
assert.equal(find("Relationships").props["aria-pressed"], true);
store.activeStoryId = "s2"; render();
for (const effect of effects) effect(); await flush(); render();
assert.equal(find("Characters").props["aria-pressed"], true);
console.log("PASS: per-story tab persistence writes only on click; story reopen collapses Reveal; unavailable storage defaults safely");
console.log("No app, backend, model, secrets, Vite server, ports, live writes, or full suites used by this probe.");
console.log("Limit: mocked hooks/actions plus real React static render, not browser interaction or integrated queue tests.");
