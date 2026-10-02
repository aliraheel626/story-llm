// Offline audit only: reads the named saved captures and writes two audit artifacts.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { isDeepStrictEqual } from 'node:util';
import { spawnSync } from 'node:child_process';

const root = dirname(fileURLToPath(import.meta.url));
const full = name => resolve(root, name);
const auditNames = ['c7-results-1-9.json', 'c7-narrator-calls.json', 'c7-evidence-audit.mjs'];
const ignorePaths = auditNames.map(name => `docs/report/entities/${name}`);
const ignoreResult = spawnSync('git', ['check-ignore', '-v', '--no-index', ...ignorePaths], { cwd: resolve(root, '../../..'), encoding: 'utf8' });
const ignoreOutput = ignoreResult.stdout ?? '';
const ignoreChecks = ignorePaths.map(path => ({ path, ignored: ignoreOutput.split('\n').some(line => line.trimEnd().endsWith(`\t${path}`)) }));
const gitIgnoreVerification = { status: ignoreResult.error ? 'NO EVIDENCE' : ignoreChecks.every(row => row.ignored) ? 'PASS' : 'FAIL', command: 'git check-ignore -v --no-index ' + ignorePaths.join(' '), exit_code: ignoreResult.status, stdout: ignoreOutput, stderr: ignoreResult.stderr ?? '', paths: ignoreChecks, note: 'Read-only check only. No ignore/config file was edited. If FAIL, the saved artifacts are currently untracked rather than ignored; coordinator was notified.' };
const ids = {
  story: '7b6992ba-996a-4b13-9750-bef67d3eb4c0',
  mira: '0831ca57-a207-40a5-97e4-10495c08c996',
  stranger: 'aac955c9-b2ac-4876-8d56-8c2cff861f97',
  tom: '0834907f-66a8-418b-abff-19d18b92e6f9',
  you: '6abab2f7-1876-4ade-9d65-6ca8d72f4751',
};
ids.relationship = `relationship:${ids.mira}:${ids.you}`;
const arrow = '\u2192';
const files = {
  setup: 'c7-00-setup.json', fresh: 'c7-01-fresh-defaults.json', empty: 'c7-01-empty-relationships.json',
  create: 'c7-02-ui-create.json', gender: 'c7-02b-gender-set.json', cleared: 'c7-02b-gender-cleared.json',
  rename: 'c7-02c-player-rename.json', initial: 'c7-03-narrator-tools.json', initialReadback: 'c7-03-entity-readback.json',
  guide: 'c7-03-guide-tools.json', verbatim: 'c7-03-verbatim-tool-events.json', stranger: 'c7-03b-stranger.json',
  revealed: 'c7-03b-revealed-name.json', apology: 'c7-04-narrator-no-stats.json', supplemental: 'c7-04-supplemental-ui-trust.json',
  context: 'c7-05-context.json', off: 'c7-05b-relationships-off-hidden.json', restored: 'c7-05b-restored.json',
  move: 'c7-06-move.json', retry: 'c7-07-retry.json', erase: 'c7-08-erase.json', roster: 'c7-08-restored-roster.json',
  eraseReadback: 'c7-08-location-link-readback.json', deleted: 'c7-09-deleted-endpoint.json',
  budgetInitial: 'c7-03-budget.json', budgetGuide: 'c7-03-guide-budget.json', budgetStranger: 'c7-03b-budget.json',
  budgetApology: 'c7-04-budget.json', budgetMove: 'c7-06-budget.json', budgetRetry: 'c7-07-budget.json',
};
const sources = [];
const data = Object.fromEntries(Object.entries(files).map(([key, name]) => {
  try {
    const bytes = readFileSync(full(name));
    const value = JSON.parse(bytes.toString('utf8'));
    sources.push({ key, path: full(name), bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
    return [key, value];
  } catch (error) {
    sources.push({ key, path: full(name), error: error.message });
    return [key, undefined];
  }
}));
const payload = entry => entry?.payload_json === undefined ? undefined : JSON.parse(entry.payload_json);
const entities = key => data[key]?.entities;
const entity = (key, id) => entities(key)?.find(row => row.id === id);
const relation = key => data[key]?.relationships?.find(row => row.entity_id === ids.relationship);
const attributes = key => data[key]?.attributes;
const trust = key => attributes(key)?.find(row => row.entity_id === ids.relationship && row.canonical_name === 'Trust');
const entries = key => data[key]?.transcript;
const events = (key, kind, id) => entries(key)?.filter(row => row.kind === kind && (id === undefined || payload(row)?.entity_id === id));
const controls = key => data[key]?.dom?.controls;
const control = (key, label) => controls(key)?.find(row => row.label === label);
const rowTexts = (key, label) => controls(key)?.filter(row => row.label === label)?.map(row => row.text);
const characterRow = (key, name) => rowTexts(key, 'Edit character')?.find(text => text.startsWith(name));
const preview = key => data[key]?.read_only_preview;
const injected = key => preview(key)?.injected;
const roster = key => injected(key)?.split('\n').filter(line => line.startsWith('- '));
const contains = (text, value) => typeof text === 'string' ? text.includes(value) : undefined;
const count = value => Array.isArray(value) ? value.length : undefined;
const toolEvents = (key, turn) => events(key, 'tool_call')?.filter(row => turn === undefined || row.turn_id === turn);
const tools = (key, turn) => toolEvents(key, turn)?.map(row => payload(row));
const input = (key, text) => entries(key)?.find(row => row.kind === 'player_message' && row.content === text);
const turnEntries = (key, turn) => turn === undefined ? undefined : entries(key)?.filter(row => row.turn_id === turn);
const narration = (key, turn) => turnEntries(key, turn)?.find(row => row.kind === 'narration');
const availableTools = key => injected(key)?.match(/Available narrator tools for this turn: ([^.\n]+)\./)?.[1]?.split(', ');
const textCalls = key => data[key]?.usage?.filter(row => ['narration', 'summary', 'title'].includes(row.kind));
const usageIds = key => data[key]?.usage?.map(row => row.id);
const narratorBoxes = key => {
  const all = controls(key);
  if (!all) return undefined;
  const start = all.findIndex(row => row.tag === 'BUTTON' && row.text === 'Narrator Tools');
  const end = all.findIndex((row, index) => index > start && row.tag === 'BUTTON' && row.text === 'Transcript');
  return start < 0 || end < 0 ? undefined : all.slice(start + 1, end).filter(row => row.type === 'checkbox').map(row => row.checked);
};
const sidebar = key => data[key]?.dom?.text?.split('\nCHARACTERS\n')[1]?.split('\nNARRATOR TOOLS\n')[0];
const facts = row => row === undefined ? undefined : Object.fromEntries(Object.entries(row).filter(([key]) => key !== 'updated_at'));
const evidence = (...keys) => keys.map(key => full(files[key]));
const missing = value => value === undefined || (Array.isArray(value) && value.some(missing)) || (value !== null && typeof value === 'object' && Object.values(value).some(missing));
const assertion = (label, actual, expected, keys, locator, match = isDeepStrictEqual) => {
  const unavailable = missing(actual) || missing(expected) || keys.some(key => data[key] === undefined);
  return { assertion: label, status: unavailable ? 'NO EVIDENCE' : match(actual, expected) ? 'PASS' : 'FAIL', expected: missing(expected) ? null : expected, actual: actual === undefined ? null : actual, actual_missing: missing(actual), evidence_full_paths: evidence(...keys), locator };
};
const eq = assertion;
const status = assertions => assertions.some(a => a.status === 'FAIL') ? 'FAIL' : assertions.some(a => a.status === 'NO EVIDENCE') ? 'NO EVIDENCE' : 'PASS';
const check = (id, title, keys, assertions, observations, notes = []) => ({
  check: id, title, status: status(assertions), evidence_full_paths: evidence(...keys), observations, assertions, notes,
});
const storyText = "My estranged sister Mira waits for me in the Rusty Anchor tavern, wearing our father's grey cloak. She still resents me for leaving.";
const guideText = 'Record Mira, where she is, what she wears, and her relationship to me, using your tools.';
const strangerText = "A hooded stranger in the corner hasn't taken her eyes off us.";
const apologyText = 'I apologize to Mira and mean it.';
const moveText = 'Mira and I step outside onto the docks.';
const firstTurn = input('initial', storyText)?.turn_id;
const guideTurn = input('guide', guideText)?.turn_id;
const strangerTurn = input('stranger', strangerText)?.turn_id;
const apologyTurn = input('apology', apologyText)?.turn_id;
const moveTurn = input('move', moveText)?.turn_id;
const retryTurn = input('retry', moveText)?.turn_id;
const setupAssertions = [
  eq('Capture is for the requested story', data.setup?.story_id, ids.story, ['setup'], 'story_id'),
  eq('Saved SQLite capture was read-only', data.setup?.database_mode, 'ro', ['setup'], 'database_mode; qa.py:25-28 opens URI mode=ro'),
  eq('Characters table exists', data.setup?.schema?.some(row => row.type === 'table' && row.name === 'characters'), true, ['setup'], 'schema (complete sqlite_master table/index capture)'),
  eq('Relationships table exists', data.setup?.schema?.some(row => row.type === 'table' && row.name === 'relationships'), true, ['setup'], 'schema'),
  eq('Legacy story_entity_state table is absent', data.setup?.schema?.some(row => row.name === 'story_entity_state'), false, ['setup'], 'schema'),
  eq('QA illustration disabled and both record tools enabled', data.setup?.tool_settings, { illustrate_scene: false, roll_check: true, save_character: true, save_relationship: true }, ['setup'], 'tool_settings'),
];
const checks = [];
checks.push(check('1', 'Fresh database and defaults', ['setup', 'fresh', 'empty'], [
  ...setupAssertions.slice(2),
  eq('Registry has no retired entity kinds', data.fresh?.registry?.flatMap(row => JSON.parse(row.entity_kinds_json)).filter(kind => ['object', 'location', 'campaign'].includes(kind)), [], ['fresh'], 'registry[*].entity_kinds_json (all captured rows)'),
  eq('Record characters/relationships checked, dice checked, illustration unchecked', narratorBoxes('fresh'), [true, true, true, false], ['fresh'], 'dom.controls between Narrator Tools and Transcript, in DOM order'),
  eq('Both entity kinds default to shown', data.fresh?.context_settings?.entity_kinds, { character: true, relationship: true }, ['fresh'], 'context_settings.entity_kinds'),
  eq('Characters shown radio checked', control('fresh', 'Characters shown')?.checked, true, ['fresh'], 'dom.controls[label=Characters shown]'),
  eq('Relationships shown radio checked', control('fresh', 'Relationships shown')?.checked, true, ['fresh'], 'dom.controls[label=Relationships shown]'),
  eq('Saved and effective settings have no removed keys', data.fresh === undefined ? undefined : [JSON.parse(data.fresh.story[0].settings_json), data.fresh.tool_settings, data.fresh.context_settings].flatMap(value => JSON.stringify(value).match(/"(?:create_entity|update_entity|get_entities|adjust_entity_attribute|entities)"\s*:/g) ?? []), [], ['fresh'], 'story[0].settings_json; effective tool_settings/context_settings'),
  eq('Transcript has no Entity lookups item', data.fresh?.transcript_settings?.filter(row => row.label === 'Entity lookups' || row.key === 'record.entity_queried'), [], ['fresh'], 'transcript_settings (all items)'),
  eq('Characters initially contains only seeded You', entities('fresh')?.map(row => ({ id: row.id, name: row.name, kind: row.kind, is_present: row.is_present })), [{ id: ids.you, name: 'You', kind: 'character', is_present: 1 }], ['fresh'], 'entities (all story rows, joined character fields)'),
  eq('Fresh character row shows You only', rowTexts('fresh', 'Edit character'), ['You'], ['fresh'], 'dom.controls[label=Edit character]'),
  eq('Relationships table initially empty', data.empty?.relationships, [], ['empty'], 'relationships (all story relationship rows)'),
  eq('Relationships tab selected and empty-state visible', [control('empty', 'Relationships')?.pressed, contains(sidebar('empty'), 'No relationships yet.')], ['true', true], ['empty'], 'dom.controls Relationships aria-pressed and character-panel text'),
], ['Fresh schema contains characters and relationships, not story_entity_state. Registry kinds are character/relationship only.', 'Effective default visibility is both shown, only You is seeded, and the selected Relationships tab is empty.']));

const tomCreated = events('create', 'entity_created', ids.tom)?.[0];
checks.push(check('2', 'UI creates character with fields', ['create'], [
  eq('Old Tom saved with requested fields', entity('create', ids.tom) === undefined ? undefined : ['name', 'role', 'location', 'outfit'].map(key => entity('create', ids.tom)[key]), ['Old Tom', 'innkeeper', 'behind the bar', 'stained apron'], ['create'], 'entities[id=Tom id]'),
  eq('Creation payload includes fields and user source', payload(tomCreated), { entity_id: ids.tom, kind: 'character', location: 'behind the bar', name: 'Old Tom', outfit: 'stained apron', role: 'innkeeper', source: 'user' }, ['create'], 'transcript[kind=entity_created,entity_id=Tom id].payload_json'),
  eq('Character row shows fields', characterRow('create', 'Old Tom'), 'Old Tombehind the barstained aproninnkeeper', ['create'], 'dom.controls[label=Edit character].text'),
], ['Old Tom is present with role innkeeper, location behind the bar and outfit stained apron. Matching entity_created is a user event.']));

const tomGenderEvents = events('cleared', 'entity_updated', ids.tom);
checks.push(check('2b', 'UI edits and clears fields', ['gender', 'cleared'], [
  eq('Gender was first stored as female', entity('gender', ids.tom)?.gender, 'female', ['gender'], 'entities[id=Tom id].gender'),
  eq('Gender is then NULL', entity('cleared', ids.tom)?.gender, null, ['cleared'], 'entities[id=Tom id].gender'),
  eq('Role/location/outfit remain unchanged across both saves', ['gender', 'cleared'].map(key => entity(key, ids.tom) === undefined ? undefined : ['role', 'location', 'outfit'].map(field => entity(key, ids.tom)[field])), [['innkeeper', 'behind the bar', 'stained apron'], ['innkeeper', 'behind the bar', 'stained apron']], ['gender', 'cleared'], 'entities[id=Tom id]'),
  eq('Exactly two updates state gender set and clear', tomGenderEvents?.map(row => row.content), [`User edit: Old Tom: gender ${arrow} female.`, 'User edit: Old Tom: gender cleared.'], ['cleared'], 'transcript entity_updated for Tom, in seq order'),
  eq('UI row no longer includes female after clearing', characterRow('cleared', 'Old Tom'), 'Old Tombehind the barstained aproninnkeeper', ['cleared'], 'dom.controls[label=Edit character]'),
], ['Gender changes female then NULL; the two actual update records state each change, and unrelated fields are preserved.']));

const renameEvent = events('rename', 'entity_updated', ids.tom)?.find(row => payload(row)?.after?.name === 'Tom');
checks.push(check('2c', 'Player edits are labelled', ['rename'], [
  eq('UI rename keeps the character id and changes name to Tom', entity('rename', ids.tom)?.name, 'Tom', ['rename'], 'entities[id=Tom id].name'),
  eq('Rename content has User edit prefix and actual rename', renameEvent?.content, `User edit: Old Tom: name Old Tom ${arrow} Tom.`, ['rename'], 'transcript rename content'),
  eq('Rename history is an authoritative record', preview('rename')?.messages?.some(row => row.role === 'record' && row.text === `[Authoritative story event: entity_updated]\n${renameEvent?.content}`), true, ['rename'], 'read_only_preview.messages'),
  eq('No QA model usage through rename', count(data.rename?.usage), 0, ['rename'], 'usage (full-table capture)'),
], ['Preview history includes the user rename as an authoritative entity_updated event; no text call was used.']));

const guideCalls = tools('guide', guideTurn);
const relationshipEvents = ['entity_created', 'entity_updated'].flatMap(kind => events('guide', kind, ids.relationship) ?? []);
checks.push(check('3', 'Narrator uses save tools after the allowed Guide', ['initial', 'initialReadback', 'guide', 'verbatim', 'budgetInitial', 'budgetGuide'], [
  eq('First Story produced no tool call', count(toolEvents('initial', firstTurn)), 0, ['initial'], 'transcript for first Story turn'),
  eq('Mira absent before Guide', entities('initial')?.filter(row => row.id === ids.mira), [], ['initial'], 'entities (all story rows)'),
  eq('Initial independent readback contains only Tom and You', data.initialReadback?.rows?.map(row => row.name).sort(), ['Tom', 'You'], ['initialReadback'], 'rows[*].name, mode=ro'),
  eq('Exactly one allowed Guide was submitted', entries('guide')?.filter(row => row.kind === 'player_message' && payload(row)?.input_mode === 'guide').map(row => row.content), [guideText], ['guide'], 'transcript player_message guide records'),
  eq('Guide calls save_character then two save_relationship calls', guideCalls?.map(call => call.tool), ['save_character', 'save_relationship', 'save_relationship'], ['guide', 'verbatim'], 'tool_call payload_json in guide turn'),
  eq('All Guide tool results succeeded', guideCalls?.map(call => call.ok), [true, true, true], ['guide', 'verbatim'], 'tool_call payload_json.ok'),
  eq('Independent verbatim tool capture matches Guide payloads exactly', data.verbatim?.rows?.filter(row => row.kind === 'tool_call').map(row => row.payload_json), toolEvents('guide', guideTurn)?.map(row => row.payload_json), ['guide', 'verbatim'], 'verbatim rows tool_call payload_json vs Guide transcript'),
  eq('Mira location/outfit match tavern/cloak', entity('guide', ids.mira) === undefined ? undefined : [entity('guide', ids.mira).location, entity('guide', ids.mira).outfit], ['the Rusty Anchor tavern', "father's grey cloak"], ['guide'], 'entities[id=Mira id]'),
  eq('Mutual creation then directed update reuse the same fixed id', relationshipEvents.map(row => ({ id: payload(row).entity_id, label: payload(row).link?.label ?? payload(row).after?.link?.label, direction: payload(row).link?.direction ?? payload(row).after?.link?.direction })), [{ id: ids.relationship, label: 'estranged sister', direction: 'both' }, { id: ids.relationship, label: 'resents', direction: 'one_way' }], ['guide', 'verbatim'], 'entity_created and entity_updated payloads for relationship id'),
  eq('Current relationship is stored Mira to You, resents, one_way', relation('guide'), { entity_id: ids.relationship, from_id: ids.mira, to_id: ids.you, label: 'resents', direction: 'one_way', description: 'She still resents you for leaving home.', is_present: 1 }, ['guide'], 'relationships'),
  eq('No removed tool name is called', guideCalls?.filter(call => ['create_entity', 'update_entity', 'get_entities', 'adjust_entity_attribute'].includes(call.tool)), [], ['guide', 'verbatim'], 'tool_call payloads'),
  eq('Attributes are still empty after Guide', attributes('guide'), [], ['guide'], 'attributes (complete captured story attributes)'),
  eq('Guide used two billed text calls', data.guide?.usage?.filter(row => row.turn_id === guideTurn).length, 2, ['guide', 'budgetGuide'], 'usage records for Guide; budget 1 to 3'),
], ['First Story attempt: NARRATOR DID NOT CALL. One permitted Guide supplied the required character fields and relationship.', 'The Guide first creates both/estranged sister and then replaces the same fixed-id link with one_way/resents, stored Mira to You. created results are true then false.', 'Two player attempts (Story plus one Guide), three billed text requests in total.'], ['The Guide reasoning text claims the second relationship failed, but saved tool results are ok=true and its update event exists; the authoritative event/result evidence takes precedence.']));

const strangerNarration = narration('stranger', strangerTurn);
checks.push(check('3b', 'True name behind title', ['stranger', 'revealed', 'budgetStranger'], [
  eq('Stranger has true name and title alias', entity('stranger', ids.stranger) === undefined ? undefined : [entity('stranger', ids.stranger).name, entity('stranger', ids.stranger).known_as], ['Isolde Vetch', 'the hooded stranger'], ['stranger'], 'entities[id=stranger id]'),
  eq('Stranger tool saves true name with known_as', tools('stranger', strangerTurn)?.map(call => [call.tool, call.args.name, call.args.known_as, call.ok]), [['save_character', 'Isolde Vetch', 'the hooded stranger', true]], ['stranger'], 'tool_call payload'),
  eq('Visible narration does not disclose either part of the true name', strangerNarration?.content === undefined ? undefined : /\b(?:Isolde|Vetch)\b/i.test(strangerNarration.content), false, ['stranger'], 'transcript narration.content only, not hidden thoughts/tool records'),
  eq('Character row title is the alias, not true name', characterRow('stranger', 'the hooded stranger') === undefined ? undefined : contains(characterRow('stranger', 'the hooded stranger'), 'Isolde Vetch'), false, ['stranger'], 'dom.controls Edit character row'),
  eq('Reveal initially collapsed', control('stranger', 'Reveal true name')?.expanded, 'false', ['stranger'], 'dom.controls[label=Reveal true name].expanded'),
  eq('Reveal pre-click sidebar excludes true name', contains(sidebar('stranger'), 'Isolde Vetch'), false, ['stranger'], 'dom.text character panel before real Reveal click'),
  eq('Post-click Reveal is open', control('revealed', 'Hide true name')?.expanded, 'true', ['revealed'], 'dom.controls[label=Hide true name].expanded'),
  eq('Post-click sidebar includes true name', contains(sidebar('revealed'), 'Isolde Vetch'), true, ['revealed'], 'dom.text character panel after real Reveal click'),
  eq('UI Reveal does not clear known_as or change stored character', entity('revealed', ids.stranger), entity('stranger', ids.stranger), ['stranger', 'revealed'], 'entities[id=stranger id] before/after UI Reveal'),
], ['Isolde Vetch is the stored true name; the player sees the hooded stranger. Visible narration omits Isolde/Vetch.', 'Saved pre/open captures show collapsed Reveal then expanded Hide true name with the true name visible; this is a UI toggle, not an in-story reveal.'], ['Behavioral status PASS; the plan\'s separate one-text-call limit is NOT met: this one player turn has two billed narration usage rows. See spending.per_step_limit.']));

const apologyStats = events('apology', 'entity_attribute_changed', ids.relationship);
const apologyCalls = tools('apology', apologyTurn);
const statsCriterion = eq('Apology called save_relationship.stats', apologyCalls?.some(call => call.tool === 'save_relationship' && Array.isArray(call.args.stats) && call.args.stats.length > 0), true, ['apology'], 'tool_call records for apology turn');
const statCheck = check('4', 'Narrator stats on relationship', ['apology', 'budgetApology'], [
  statsCriterion,
  eq('Apology relationship attribute changes exist', count(apologyStats), 1, ['apology'], 'full transcript entity_attribute_changed records for relationship'),
  eq('Relationship has an attribute after apology', count(attributes('apology')?.filter(row => row.entity_id === ids.relationship)), 1, ['apology'], 'attributes'),
], ['Apology calls only roll_check. It returns failure (roll 54, chance 35); no save_relationship call, stats argument, attribute event or attribute row exists.', 'The original narrator-stat pass condition is not satisfied. Later player Trust edits are supplemental UI/context coverage only.']);
statCheck.status = statsCriterion.status === 'NO EVIDENCE' ? 'NO EVIDENCE' : statsCriterion.actual === false && count(apologyStats) === 0 && count(attributes('apology')) === 0 ? "NARRATOR DIDN'T CALL" : statCheck.status;
checks.push(statCheck);

const supplementalEvents = events('supplemental', 'entity_attribute_changed', ids.relationship);
const supplemental = check('4-supplemental', 'Normal UI Trust coverage, not narrator stats', ['apology', 'supplemental'], [
  eq('UI adds Trust 0 then sets -2 as exactly two user events', supplementalEvents?.map(row => ({ before: payload(row).before, after: payload(row).after, source: payload(row).source, turn_id: row.turn_id })), [{ before: null, after: 0, source: 'user', turn_id: null }, { before: 0, after: -2, source: 'user', turn_id: null }], ['supplemental'], 'transcript relationship entity_attribute_changed records'),
  eq('Stored Trust -2 is a user value', trust('supplemental') === undefined ? undefined : [trust('supplemental').value, trust('supplemental').source], [-2, 'user'], ['supplemental'], 'attributes[canonical_name=Trust]'),
  eq('Saved relationship row shows inline Trust -2', rowTexts('supplemental', 'Edit relationship'), [`Mira ${arrow} YouresentsShe still resents you for leaving home.Trust -2`], ['supplemental'], 'dom.controls[label=Edit relationship]'),
  eq('Supplemental UI actions add no model usage', usageIds('supplemental'), usageIds('apology'), ['apology', 'supplemental'], 'usage ids unchanged'),
], ['Normal UI Add created Trust at 0, then blur/Save set -2. Exactly two user-source events exist; there is no duplicate unchanged Save event.', 'This demonstrates player stat editing and inline display, not narrator save_relationship.stats.']);

checks.push(check('5', 'Preview sees current record', ['supplemental', 'context'], [
  eq('Entities roster contains directed Mira relationship with Trust -2', roster('context')?.find(line => line.startsWith(`- Mira ${arrow} You:`)), `- Mira ${arrow} You: resents (relationship); description: She still resents you for leaving home.; attributes: Trust=-2`, ['context'], 'read_only_preview.injected roster line'),
  eq('Entities roster carries Mira tavern and cloak', roster('context')?.some(line => line.startsWith('- Mira (character);') && line.includes('location: the Rusty Anchor tavern;') && line.includes("outfit: father's grey cloak;")), true, ['context'], 'read_only_preview.injected Mira character line'),
  eq('Stranger roster carries known-to-player alias', roster('context')?.some(line => line.startsWith('- Isolde Vetch (character); known to the player as: the hooded stranger;')), true, ['context'], 'read_only_preview.injected stranger character line'),
  eq('UI next-request preview opened', contains(data.context?.dom?.text, 'System: # Role') && contains(data.context?.dom?.text, 'record: [Authoritative story event:'), true, ['context'], 'dom.text Preview next request rendered system/history'),
  eq('No extra model call for preview', usageIds('context'), usageIds('supplemental'), ['supplemental', 'context'], 'usage ids unchanged, seven text calls'),
], ['Preview uses the supplemental player-set Trust -2 for attribute coverage; it does not retroactively pass check 4.', 'Both the rendered request preview and saved read_only_preview expose the current roster without making an additional LLM call.']));

checks.push(check('5b', 'Record and Show switches off and restored', ['context', 'off', 'restored'], [
  eq('Relationship recording is saved false', data.off?.tool_settings?.save_relationship, false, ['off'], 'tool_settings.save_relationship'),
  eq('Relationship visibility is saved false', data.off?.context_settings?.entity_kinds?.relationship, false, ['off'], 'context_settings.entity_kinds.relationship'),
  eq('Persisted settings also store both false', data.off?.story?.[0] === undefined ? undefined : [JSON.parse(data.off.story[0].settings_json).narrator_tools.save_relationship, JSON.parse(data.off.story[0].settings_json).context.entity_kinds.relationship], [false, false], ['off'], 'story[0].settings_json'),
  eq('Relationship save tool omitted from advertised preview tools', availableTools('off'), ['roll_check', 'save_character'], ['off'], 'read_only_preview.injected Available narrator tools line'),
  eq('No relationship roster line while hidden', roster('off')?.filter(line => line.startsWith(`- Mira ${arrow} You`)), [], ['off'], 'read_only_preview.injected lines starting - only'),
  eq('Mira character roster line remains', roster('off')?.some(line => line.startsWith('- Mira (character);')), true, ['off'], 'read_only_preview.injected roster'),
  eq('Record off does not delete stored relationship or stats', [relation('off'), trust('off')], [relation('context'), trust('context')], ['context', 'off'], 'relationships and attributes'),
  eq('Record and Show are restored true', data.restored === undefined ? undefined : [data.restored.tool_settings.save_relationship, data.restored.context_settings.entity_kinds.relationship], [true, true], ['restored'], 'tool_settings and context_settings'),
  eq('Entire read-only preview matches check 5 after restore', preview('restored'), preview('context'), ['context', 'restored'], 'read_only_preview system/injected/messages/images_unsupported'),
  eq('Switching off/restoring adds no model calls', [usageIds('off'), usageIds('restored')], [usageIds('context'), usageIds('context')], ['context', 'off', 'restored'], 'usage ids'),
], ['Both independent saved switches become false. The preview advertises only roll_check/save_character, removes the relationship roster line, and retains Mira.', 'Restoring both true gives an identical preview to check 5. Header arrows and historical relationship events are intentionally not treated as visible roster lines.']));

const moveMiraEvent = turnEntries('move', moveTurn)?.find(row => row.kind === 'entity_updated' && payload(row)?.entity_id === ids.mira);
checks.push(check('6', 'Moving to docks', ['move', 'budgetMove'], [
  eq('Move input is the planned Do action', payload(input('move', moveText))?.input_mode, 'do', ['move'], 'transcript player_message'),
  eq('Mira current location is recorded at docks', entity('move', ids.mira)?.location, 'the docks outside the Rusty Anchor', ['move'], 'entities[id=Mira id].location'),
  eq('Mira update explicitly states tavern to docks', moveMiraEvent?.content, `Mira: location the Rusty Anchor tavern ${arrow} the docks outside the Rusty Anchor.`, ['move'], 'move turn entity_updated content'),
  eq('Mira location update event has matching before/after', payload(moveMiraEvent) === undefined ? undefined : [payload(moveMiraEvent).before.location, payload(moveMiraEvent).after.location], ['the Rusty Anchor tavern', 'the docks outside the Rusty Anchor'], ['move'], 'move turn entity_updated payload'),
  eq('Mira UI row shows docks', contains(characterRow('move', 'Mira'), 'the docks outside the Rusty Anchor'), true, ['move'], 'dom.controls Edit character Mira row'),
  eq('Narration describes docks', contains(narration('move', moveTurn)?.content, 'the docks'), true, ['move'], 'move turn narration.content'),
], ['The narrator saves Mira and You at the docks; Mira\'s event, current row and UI row match. The relationship and user Trust -2 are unchanged.']));

const originalMoveEntries = turnEntries('move', moveTurn);
const replacementEntries = turnEntries('retry', retryTurn);
checks.push(check('7', 'Retry replaces move records', ['move', 'retry', 'budgetRetry'], [
  eq('Retried turn has a different id', retryTurn === undefined || moveTurn === undefined ? undefined : retryTurn !== moveTurn, true, ['move', 'retry'], 'planned move player_message.turn_id before/after'),
  eq('No original move transcript entry survives retry', originalMoveEntries === undefined ? undefined : entries('retry')?.filter(row => originalMoveEntries.some(old => old.id === row.id)).map(row => row.id), [], ['move', 'retry'], 'full captured transcript id comparison, including original player/narration/records'),
  eq('No old move turn remains in transcript', count(turnEntries('retry', moveTurn)), 0, ['retry'], 'transcript entries matching original turn_id'),
  eq('Earlier history is unchanged by replacement', entries('retry')?.filter(row => row.seq < input('retry', moveText)?.seq), entries('move')?.filter(row => row.seq < input('move', moveText)?.seq), ['move', 'retry'], 'complete transcript records before planned move input'),
  eq('Exactly one replacement narration remains', count(replacementEntries?.filter(row => row.kind === 'narration')), 1, ['retry'], 'retry turn transcript'),
  eq('Only replacement narration, updates and calls occupy the new suffix', replacementEntries?.map(row => row.kind), ['player_message', 'narration', 'entity_updated', 'entity_updated', 'entity_updated', 'tool_call', 'tool_call', 'tool_call'], ['retry'], 'retry transcript suffix in seq order'),
  eq('Mira current location is unchanged docks and matches retry tool', entity('retry', ids.mira)?.location, entity('move', ids.mira)?.location, ['move', 'retry'], 'entities Mira location before/after retry'),
  eq('Retry narration describes docks', contains(narration('retry', retryTurn)?.content, 'The docks'), true, ['retry'], 'replacement narration.content'),
  eq('Retry saves Mira location to docks', tools('retry', retryTurn)?.find(call => call.tool === 'save_character' && call.args.name === 'Mira')?.args.location, entity('retry', ids.mira)?.location, ['retry'], 'replacement tool_call vs stored entity'),
  eq('Relationship and Trust are identical after retry', [relation('retry'), trust('retry')], [relation('move'), trust('move')], ['move', 'retry'], 'relationships and attributes'),
], ['Original move turn fc6cbc6e-8291-4932-96d2-929792b6a55d is replaced by 9849e3a1-53e4-4152-9d1d-416903b366bb; old transcript record ids do not survive.', 'Retry saves Mira/You at the docks and the stranger at the doorway, matching the replacement narration.'], ['Earlier-attempt usage rows are retained and reassigned to the replacement turn for cost accounting; they are not surviving old transcript records.']));

checks.push(check('8', 'Erase restores tavern and preserves relationship', ['retry', 'erase', 'roster', 'eraseReadback'], [
  eq('Erased retry turn has no remaining transcript rows', count(turnEntries('erase', retryTurn)), 0, ['erase'], 'full transcript for replacement move turn_id'),
  eq('Full transcript returns to pre-move records', entries('erase'), entries('context'), ['context', 'erase'], 'complete saved transcript; user Trust edits remain'),
  eq('Mira restores tavern location', entity('erase', ids.mira)?.location, 'the Rusty Anchor tavern', ['erase'], 'entities Mira location'),
  eq('Other Mira fields and identity match pre-move state', facts(entity('erase', ids.mira)), facts(entity('context', ids.mira)), ['context', 'erase'], 'entities Mira, ignoring rebuilt updated_at only'),
  eq('You and stranger locations also restore', [entity('erase', ids.you)?.location, entity('erase', ids.stranger)?.location], [null, 'corner of the Rusty Anchor'], ['erase'], 'entities You/stranger locations'),
  eq('Relationship row unchanged through endpoint replay', relation('erase'), relation('retry'), ['retry', 'erase'], 'relationships complete row including id, endpoints, label, direction, presence'),
  eq('Trust -2 unchanged through erase', trust('erase'), trust('retry'), ['retry', 'erase'], 'attributes full Trust row, including value/source/updated_at'),
  eq('Expected open unsaved edit draft retains docks input', control('erase', 'Location')?.value, 'the docks outside the Rusty Anchor', ['erase'], 'dom.controls Location input in previously-open Mira edit card'),
  eq('After cancelling draft, Mira UI row displays tavern', contains(characterRow('roster', 'Mira'), 'the Rusty Anchor tavern'), true, ['roster'], 'dom.controls Edit character Mira row after Cancel'),
  eq('Cancelled edit card has no remaining Location input', controls('roster')?.filter(row => row.label === 'Location'), [], ['roster'], 'dom.controls'),
  eq('Cancel does not create a data edit', entries('roster')?.map(row => row.id), entries('erase')?.map(row => row.id), ['erase', 'roster'], 'transcript ids identical'),
  eq('Independent saved readback confirms tavern, resents, Trust -2', data.eraseReadback?.rows?.find(row => row.name === 'Mira'), { name: 'Mira', location: 'the Rusty Anchor tavern', label: 'resents', trust: -2 }, ['eraseReadback'], 'rows[name=Mira], mode=ro'),
], ['Erase restores Mira to the tavern; You location is NULL and the stranger is back in the corner. Relationship identity/label/direction and user Trust -2 remain unchanged.', 'The initially open edit card intentionally retains its unsaved docks draft against its edit-open baseline. After Cancel, the actual roster row shows the restored tavern. This is not an app-data mismatch.']));

checks.push(check('9', 'Deleting endpoint hides relationship without deleting stored link', ['roster', 'deleted'], [
  eq('Mira is soft-deleted, not physically absent', entity('deleted', ids.mira)?.is_present, 0, ['deleted'], 'entities[id=Mira id].is_present'),
  eq('Stored relationship still exists and is present', relation('deleted'), relation('roster'), ['roster', 'deleted'], 'relationships full row, is_present=1'),
  eq('Relationship entity presence remains 1', entity('deleted', ids.relationship)?.is_present, 1, ['deleted'], 'entities[id=relationship id].is_present'),
  eq('Relationships tab is selected', control('deleted', 'Relationships')?.pressed, 'true', ['deleted'], 'dom.controls Relationships aria-pressed'),
  eq('Reloaded UI has no relationship rows', rowTexts('deleted', 'Edit relationship'), [], ['deleted'], 'dom.controls Edit relationship buttons'),
  eq('Empty Relationships UI message is visible', contains(sidebar('deleted'), 'No relationships yet.'), true, ['deleted'], 'dom.text character panel'),
  eq('Preview roster has no Mira character or Mira to You relationship', roster('deleted')?.filter(line => line.startsWith('- Mira (character)') || line.startsWith(`- Mira ${arrow} You:`)), [], ['deleted'], 'read_only_preview.injected roster lines only'),
  eq('Relationship Trust -2 is still stored', trust('deleted'), trust('roster'), ['roster', 'deleted'], 'attributes Trust row'),
  eq('Deletion is a user event', events('deleted', 'entity_deleted', ids.mira)?.map(row => payload(row).source), ['user'], ['deleted'], 'transcript entity_deleted Mira payload'),
], ['Mira remains stored with is_present=0; her fixed-id relationship and Trust -2 remain stored/present, but the reloaded Relationships tab and preview roster omit them.'], ['Historical authoritative messages may still contain Mira and old relationship names; presence filtering is for the current roster, not deletion of history.']));

const latestUsage = data.deleted?.usage;
const sumCost = rows => rows === undefined ? undefined : Math.round(rows.reduce((sum, row) => sum + (row.cost_usd ?? 0), 0) * 1e6) / 1e6;
const spendingAssertions = [
  eq('Latest full captured usage has eleven text calls', count(textCalls('deleted')), 11, ['deleted', 'budgetRetry'], 'usage: narration + summary + title, including earlier attempts'),
  eq('Latest full captured text cost is $0.110782', sumCost(textCalls('deleted')), 0.110782, ['deleted', 'budgetRetry'], 'usage.cost_usd sum (rounded to six decimals)'),
  eq('Global text cap of fifteen is respected', latestUsage === undefined ? undefined : count(textCalls('deleted')) <= 15, true, ['deleted', 'budgetRetry'], 'text usage count'),
  eq('No image or caption calls', latestUsage?.filter(row => ['image', 'caption'].includes(row.kind)), [], ['deleted', 'budgetRetry'], 'usage (full-table capture)'),
  eq('Title/summary usage is zero', latestUsage?.filter(row => ['title', 'summary'].includes(row.kind)), [], ['deleted'], 'usage; story title is already Entities QA in setup'),
  eq('All saved application usage belongs to the QA story', latestUsage === undefined ? undefined : [...new Set(latestUsage.map(row => row.story_id))], [ids.story], ['deleted'], 'usage[*].story_id; captured application data only'),
  eq('Every billed request has a known numeric cost', latestUsage?.every(row => typeof row.cost_usd === 'number'), true, ['deleted'], 'usage[*].cost_usd; no inferred zero for missing costs'),
  eq('Budget snapshot agrees with independently counted saved usage', data.budgetRetry === undefined ? undefined : [data.budgetRetry.text_calls, data.budgetRetry.image_calls, data.budgetRetry.caption_calls, data.budgetRetry.usage_totals[0].cost_usd], [11, 0, 0, 0.110782], ['budgetRetry', 'deleted'], 'budget totals vs saved full usage'),
];
const perStepLimit = eq('C7-3b plan specifies one text call; actual billed requests are counted', data.stranger?.usage?.filter(row => row.turn_id === strangerTurn && ['narration', 'summary', 'title'].includes(row.kind)).length, 1, ['stranger', 'budgetStranger'], 'usage for stranger turn; plan entities.md:560');
const strangerCheck = checks.find(row => row.check === '3b');
strangerCheck.behavioral_status = strangerCheck.status;
strangerCheck.procedure_status = perStepLimit.status;
strangerCheck.assertions.push(perStepLimit);
strangerCheck.status = status(strangerCheck.assertions);
strangerCheck.failure_category = perStepLimit.status === 'FAIL' && strangerCheck.behavioral_status === 'PASS' ? 'procedure' : null;
const callStages = [
  ['3 first Story', 'initial', firstTurn], ['3 single Guide', 'guide', guideTurn], ['3b stranger', 'stranger', strangerTurn],
  ['4 apology', 'apology', apologyTurn], ['6 move', 'move', moveTurn], ['7 retry replacement', 'retry', retryTurn],
].map(([stage, key, turn]) => ({ stage, evidence_full_path: full(files[key]), turn_id: turn, calls: data[key]?.usage?.filter(row => row.turn_id === turn && row.earlier_attempt === 0).length, note: 'One usage row per billed provider request; tool continuation requests count separately.' }));

const extractCall = (key, row) => {
  const value = payload(row);
  const argsJson = JSON.stringify(value.args);
  const resultJson = JSON.stringify(value.result);
  return {
    evidence_full_path: full(files[key]), transcript_entry_id: row.id, seq: row.seq, turn_id: row.turn_id, target_entry_id: row.target_entry_id,
    content_verbatim: row.content, payload_json_verbatim: row.payload_json,
    tool: value.tool, ok: value.ok, args: value.args, result: value.result,
    args_verbatim_json: row.payload_json.includes(`"args":${argsJson}`) ? argsJson : null,
    result_verbatim_json: row.payload_json.includes(`"result":${resultJson}`) ? resultJson : null,
  };
};
const phase = (label, key, turn) => ({
  phase: label, evidence_full_paths: evidence(key), turn_id: turn,
  input: entries(key)?.find(row => row.kind === 'player_message' && row.turn_id === turn)?.content,
  tool_calls: toolEvents(key, turn)?.map(row => extractCall(key, row)),
  narration: narration(key, turn) === undefined ? null : { transcript_entry_id: narration(key, turn).id, content_verbatim: narration(key, turn).content },
  billed_text_calls: data[key]?.usage?.filter(row => row.turn_id === turn).length,
});
const narratorAssertions = {
  true_names: [eq('Character tools use true names', [guideCalls?.find(call => call.tool === 'save_character')?.args.name, tools('stranger', strangerTurn)?.[0]?.args.name], ['Mira', 'Isolde Vetch'], ['guide', 'stranger'], 'tool_call args.name'), eq('Relationship calls reference named Mira and You endpoints', guideCalls?.filter(call => call.tool === 'save_relationship').map(call => [call.args.from, call.args.to]), [['Mira', 'You'], ['Mira', 'You']], ['guide'], 'tool_call args.from/to')],
  known_as: strangerCheck.assertions.filter(row => ['Stranger has true name and title alias', 'Stranger tool saves true name with known_as', 'Visible narration does not disclose either part of the true name'].includes(row.assertion)),
  labels_and_directions: checks.find(row => row.check === '3').assertions.filter(row => ['Mutual creation then directed update reuse the same fixed id', 'Current relationship is stored Mira to You, resents, one_way'].includes(row.assertion)),
  removed_tools: [eq('No removed tool called in either checked turn', [...(guideCalls ?? []), ...(tools('stranger', strangerTurn) ?? [])].filter(call => ['create_entity', 'update_entity', 'get_entities', 'adjust_entity_attribute'].includes(call.tool)), [], ['guide', 'stranger'], 'tool_call names in both turns')],
  no_stats_before_supplemental: [eq('Attributes absent through Story, Guide, stranger and apology', ['initial', 'guide', 'stranger', 'apology'].map(key => count(attributes(key))), [0, 0, 0, 0], ['initial', 'guide', 'stranger', 'apology'], 'attributes arrays'), eq('No stats supplied by Guide or stranger tool calls', [...(guideCalls ?? []), ...(tools('stranger', strangerTurn) ?? [])].filter(call => Object.hasOwn(call.args, 'stats')), [], ['guide', 'stranger'], 'tool_call args stats key'), ...supplemental.assertions.slice(0, 2)],
};
const narratorArtifact = {
  audit_scope: 'Offline saved-evidence extraction for checks 3 and 3b only; no app interaction, provider requests or DB access.',
  ids, plan_full_path: resolve(root, '../../plans/entities.md'),
  phases: [phase('3 first Story: narrator did not call', 'initial', firstTurn), phase('3 allowed single Guide', 'guide', guideTurn), phase('3b hooded stranger', 'stranger', strangerTurn)],
  assessment: {
    true_names: { status: status(narratorAssertions.true_names), assertions: narratorAssertions.true_names, observation: 'save_character names Mira and Isolde Vetch; Mira/You endpoints use actual character names, not titles or invented ids.' },
    known_as: { status: status(narratorAssertions.known_as), assertions: narratorAssertions.known_as, observation: 'Isolde Vetch is saved with known_as=the hooded stranger; visible narration uses hood/figure descriptions and never Isolde or Vetch.' },
    labels_and_directions: { status: status(narratorAssertions.labels_and_directions), assertions: narratorAssertions.labels_and_directions, observation: 'both/estranged sister is sensible for siblings; the next one_way/resents is sensible for Mira\'s directed feeling. It updates the same fixed id, not a second row. Current orientation is Mira to You. The mutual sibling label is replaced, not retained as a separate relationship.' },
    removed_tools: { status: status(narratorAssertions.removed_tools), assertions: narratorAssertions.removed_tools, observation: 'Calls are save_character/save_relationship only; no create_entity/update_entity/get_entities/adjust_entity_attribute.' },
    no_stats_before_supplemental: { status: status(narratorAssertions.no_stats_before_supplemental), assertions: narratorAssertions.no_stats_before_supplemental, observation: 'No stats in these calls and attributes arrays are empty through the apology capture. Trust arrives only in later source=user events.' },
    narration_quality_note: 'Stranger narration begins with meta text about putting the watcher in the record; this is not a true-name leak and is not the entity-check pass condition.',
    authoritative_results_note: 'Guide thoughts claim a failure, but all three saved tool events have ok=true. Both creation and directed update events confirm success.',
  },
  relationship_identity: { id: ids.relationship, created_direction: 'both', created_label: 'estranged sister', updated_direction: 'one_way', updated_label: 'resents', from_id: ids.mira, to_id: ids.you, evidence_full_paths: evidence('guide', 'verbatim') },
  reveal: { pre_evidence_full_path: full(files.stranger), post_evidence_full_path: full(files.revealed), pre_expanded: control('stranger', 'Reveal true name')?.expanded, post_expanded: control('revealed', 'Hide true name')?.expanded, stored_known_as_after_ui_reveal: entity('revealed', ids.stranger)?.known_as },
  counted_provider_calls: { first_story: callStages[0].calls, guide: callStages[1].calls, stranger: callStages[2].calls, note: 'One Guide/player turn is not one provider call. Stranger one-text-call plan limit is a separately recorded deviation.' },
  verbatim_contract: 'payload_json_verbatim is the exact saved DB record value. args/result are unchanged decoded objects, and args_verbatim_json/result_verbatim_json are verified substrings of that saved payload; no claim is made about an uncaptured raw provider wire request.',
};

const auditArtifact = {
  audit_scope: 'Read-only audit of named saved C7 evidence, with artifact-only writes. No app interaction/model request/SQLite connection/DB mutation/app-source edit/commit/secret access.',
  ids, plan_full_path: resolve(root, '../../plans/entities.md'),
  artifact_git_ignore_verification: gitIgnoreVerification,
  capture_provenance: { capture_helper_full_path: full('qa.py'), read_only_query_helper_full_path: full('qa-db.py'), observation: 'qa.py:25-28 and qa-db.py:16 open SQLite URI mode=ro. Full captures explicitly record database_mode=ro; standalone readbacks and budget captures record mode=ro. No new SQL was needed or executed in this audit.', assertions: Object.keys(files).map(key => eq('Saved DB capture/readback is labelled ro', data[key]?.database_mode ?? data[key]?.mode, 'ro', [key], 'database_mode or mode')) },
  setup: { status: status(setupAssertions), evidence_full_paths: evidence('setup'), assertions: setupAssertions },
  checks,
  supplemental_checks: [supplemental],
  deferred_checks: [
    { check: '10', status: 'NO EVIDENCE', reason: 'Persistence restart is outside this 1-9 audit and was still pending at assignment; coordinator owns later collection. Not inferred PASS from earlier captures.' },
    { check: '11', status: 'NO EVIDENCE', reason: 'Final backend/frontend log audit is outside this 1-9 audit and was still pending at assignment; coordinator owns later collection. Existing incidental logs do not substitute for that check.' },
  ],
  spending: { status: status(spendingAssertions), text_calls: count(textCalls('deleted')), text_cap: 15, text_calls_remaining: 15 - count(textCalls('deleted')), text_cost_usd: sumCost(textCalls('deleted')), total_cost_usd: sumCost(latestUsage), images: count(latestUsage?.filter(row => row.kind === 'image')), captions: count(latestUsage?.filter(row => row.kind === 'caption')), title_calls: count(latestUsage?.filter(row => row.kind === 'title')), summary_calls: count(latestUsage?.filter(row => row.kind === 'summary')), stages: callStages, assertions: spendingAssertions, per_step_limit: perStepLimit, notes: ['Story manually titled Entities QA before narration; saved usage confirms no title request. No coding-assistant/subagent calls are included, only captured application usage_records.', 'Retried/erased turns do not refund calls: all eleven captured billed requests, including earlier_attempt=1 rows, remain counted.', 'Global 15/0/0 spending cap PASS; literal C7-3b one-text-call sublimit FAIL (actual 2). This is a test-procedure spending deviation, not an entity-model data failure.'] },
  summary: { check_count_1_to_9_including_lettered_checks: checks.length, status_counts: Object.fromEntries(['PASS', 'FAIL', 'NO EVIDENCE', "NARRATOR DIDN'T CALL"].map(value => [value, checks.filter(row => row.status === value).length])), overall_failures: checks.filter(row => row.status === 'FAIL').map(row => row.check), behavioral_failures: checks.filter(row => row.status === 'FAIL' && row.failure_category !== 'procedure').map(row => row.check), behavioral_no_evidence: checks.filter(row => row.status === 'NO EVIDENCE').map(row => row.check), narrator_did_not_call: checks.filter(row => row.status === "NARRATOR DIDN'T CALL").map(row => row.check), procedure_failures: perStepLimit.status === 'FAIL' ? ['C7-3b one-text-call sublimit: actual two billed requests; behavioral status PASS'] : [] },
  notes: ['Missing input files/fields produce NO EVIDENCE, never a default PASS. Assertion actual/expected values are extracted from the named saved files.', 'This audit validates saved snapshots/events, not uncaptured transport details or a new replay of the user clicks.', 'Narrator #4 remains NARRATOR DIDN\'T CALL despite supplemental Trust UI coverage. Preview #5 uses source=user Trust -2 without another LLM call.', 'Post-erase open edit draft is intentionally retained until Cancel; restored roster and independent saved readback prove actual tavern data.', 'Deleted endpoints are filtered from current UI/roster only. Names in historical messages are permitted.', 'Artifact Git-ignore verification is a separate read-only Git check; the auditor does not change repository ignore/config/source files or the parent-owned docs/report/entities.md.'],
  source_manifest: sources,
  related_artifacts_full_paths: [full('c7-narrator-calls.json'), full('c7-evidence-audit.mjs')],
};

const asciiJson = value => JSON.stringify(value, null, 2).replace(/[^\x00-\x7f]/g, char => `\\u${char.charCodeAt(0).toString(16).padStart(4, '0')}`) + '\n';
for (const [name, value] of [['c7-results-1-9.json', auditArtifact], ['c7-narrator-calls.json', narratorArtifact]]) {
  if (process.argv.includes('--verify')) {
    if (!isDeepStrictEqual(JSON.parse(readFileSync(full(name), 'utf8')), value)) throw new Error(`Saved audit differs from recomputed evidence: ${name}`);
    console.log(`Verified ${full(name)}`);
  } else {
    writeFileSync(full(name), asciiJson(value), 'utf8');
    console.log(full(name));
  }
}
if (sources.some(row => row.error)) throw new Error('Missing or invalid evidence source; audit artifacts record NO EVIDENCE.');
for (const stage of narratorArtifact.phases) for (const call of stage.tool_calls ?? []) {
  if (call.args_verbatim_json === null || call.result_verbatim_json === null) throw new Error(`Verbatim payload substring extraction failed: ${call.transcript_entry_id}`);
}
console.log(JSON.stringify({ summary: auditArtifact.summary, artifact_ignore_status: gitIgnoreVerification.status, global_spending: auditArtifact.spending.status, per_step_limit: perStepLimit, failed_assertions: checks.flatMap(row => row.assertions.filter(a => a.status !== 'PASS').map(a => ({ check: row.check, check_status: row.status, assertion: a.assertion, actual: a.actual, expected: a.expected }))) }, null, 2));
