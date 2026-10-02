import { useEffect, useState } from "react";
import { CHARACTER_FIELD_LABELS } from "../../shared/types";
import type { AttributeRegistryEntry, CharacterFields, CharacterPatch, Entity, EntityAttributeValue } from "../../shared/types";
import { useStoryStore } from "../story/store";
import { charactersApi } from "./api";

const EMPTY_ENTITIES: Entity[] = [];
const EMPTY_ATTRIBUTES: EntityAttributeValue[] = [];
const EMPTY_ATTRIBUTE_MAP: Record<string, EntityAttributeValue[]> = {};
const CHARACTER_KEYS = Object.keys(CHARACTER_FIELD_LABELS) as (keyof CharacterFields)[];
const FIELD_CLASS = "w-full rounded border border-border bg-surface px-2 py-1 text-xs text-text focus:outline-none focus:border-accent disabled:opacity-40";
type CharacterDrafts = Record<keyof CharacterFields, string>;
type CharacterForm = { token: symbol; name: string; fields: CharacterDrafts; busy: boolean };
type EditForm = CharacterForm & {
  original: Entity;
  attributeDrafts: Record<string, { value: string }>;
  removedAttributes: string[];
  attributeToAdd: string;
};

const preferredTab = (storyId: string | null): Entity["kind"] => {
  try {
    if (storyId && window.localStorage.getItem(`story-llm:entity-tab:${storyId}`) === "relationship") return "relationship";
  } catch { /* UI preferences are optional when storage is unavailable. */ }
  return "character";
};

const characterDrafts = (entity?: CharacterFields): CharacterDrafts =>
  Object.fromEntries(CHARACTER_KEYS.map((field) => [field, entity?.[field] ?? ""])) as CharacterDrafts;

const changedFields = (drafts: CharacterDrafts, original?: CharacterFields): CharacterPatch => {
  const patch: CharacterPatch = {};
  for (const field of CHARACTER_KEYS) {
    const value = drafts[field].trim() || null;
    if (value !== (original?.[field] ?? null)) patch[field] = value;
  }
  return patch;
};

export function CharactersPanel() {
  const activeStoryId = useStoryStore((s) => s.activeStoryId);
  const entities = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.entities ?? EMPTY_ENTITIES) : EMPTY_ENTITIES);
  const loading = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.entitiesLoading ?? false) : false);
  const attributesByEntity = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.attributesByEntity ?? EMPTY_ATTRIBUTE_MAP) : EMPTY_ATTRIBUTE_MAP);
  const loadEntities = useStoryStore((s) => s.loadEntities);
  const createEntity = useStoryStore((s) => s.createEntity);
  const updateEntity = useStoryStore((s) => s.updateEntity);
  const deleteEntity = useStoryStore((s) => s.deleteEntity);
  const setEntityAttribute = useStoryStore((s) => s.setEntityAttribute);
  const removeEntityAttribute = useStoryStore((s) => s.removeEntityAttribute);

  const [tab, setTab] = useState<Entity["kind"]>("character");
  const [creating, setCreating] = useState<CharacterForm | null>(null);
  const [editing, setEditing] = useState<EditForm | null>(null);
  const [revealedIds, setRevealedIds] = useState<string[]>([]);
  const [registry, setRegistry] = useState<AttributeRegistryEntry[]>([]);

  useEffect(() => {
    setTab(preferredTab(activeStoryId));
    setCreating(null);
    setEditing(null);
    setRevealedIds([]);
    if (activeStoryId) loadEntities(activeStoryId).catch(console.error);
  }, [activeStoryId, loadEntities]);
  useEffect(() => { charactersApi.listRegistry().then(setRegistry).catch(console.error); }, []);

  if (!activeStoryId) return <div className="text-xs text-muted py-1">Open a story first.</div>;

  const updateEdit = (form: EditForm, change: (current: EditForm) => EditForm | null) =>
    setEditing((current) => current?.token === form.token ? change(current) : current);
  const attributes = editing ? (attributesByEntity[editing.original.id] ?? EMPTY_ATTRIBUTES) : EMPTY_ATTRIBUTES;
  const availableAttributes = editing ? registry.filter((definition) =>
    !attributes.some((item) => item.attribute_id === definition.id) && !editing.attributeDrafts[definition.id]
    && JSON.parse(definition.entity_kinds_json).includes(editing.original.kind)) : [];
  const endpointName = (id: string) => {
    const endpoint = entities.find((entity) => entity.id === id && entity.kind === "character");
    return endpoint ? (endpoint.known_as ?? endpoint.name) : "Unknown character";
  };
  const relationshipTitle = (entity: Entity) => entity.link
    ? `${endpointName(entity.link.from_id)} ${entity.link.direction === "both" ? "\u2194" : "\u2192"} ${endpointName(entity.link.to_id)}`
    : "Relationship";
  const renderFields = (form: CharacterForm, onChange: (field: keyof CharacterFields, value: string) => void) =>
    CHARACTER_KEYS.map((field) => (
      <label key={field} className="flex flex-col gap-1 text-xs text-muted">{CHARACTER_FIELD_LABELS[field]}
        {field === "appearance_anchor" ? (
          <textarea aria-label={CHARACTER_FIELD_LABELS[field]} value={form.fields[field]} disabled={form.busy} rows={3}
            onChange={(event) => onChange(field, event.target.value)} className={`${FIELD_CLASS} resize-none`} />
        ) : (
          <input aria-label={CHARACTER_FIELD_LABELS[field]} value={form.fields[field]} disabled={form.busy}
            onChange={(event) => onChange(field, event.target.value)} className={FIELD_CLASS} />
        )}
      </label>
    ));

  const submitNew = async () => {
    const form = creating;
    if (!form || form.busy || !form.name.trim()) return;
    setCreating((current) => current?.token === form.token ? { ...current, busy: true } : current);
    try {
      await createEntity(activeStoryId, form.name.trim(), changedFields(form.fields));
      setCreating((current) => current?.token === form.token ? null : current);
    } catch (error) { console.error(error); }
    finally { setCreating((current) => current?.token === form.token ? { ...current, busy: false } : current); }
  };

  const startEdit = (entity: Entity) => {
    setEditing({
      token: Symbol(), original: { ...entity }, name: entity.kind === "character" ? entity.name : (entity.link?.label ?? ""),
      fields: characterDrafts(entity), busy: false, attributeDrafts: {}, removedAttributes: [], attributeToAdd: "",
    });
  };

  const submitEdit = async () => {
    const form = editing;
    if (!form || form.busy || !form.name.trim()) return;
    const originalName = form.original.kind === "character" ? form.original.name : form.original.link?.label;
    const name = form.name.trim() === originalName ? undefined : form.name.trim();
    // Keep the edit-open baseline: stat reloads may contain unrelated, newer narration facts.
    const fields = form.original.kind === "character" ? changedFields(form.fields, form.original) : {};
    const drafts = Object.fromEntries(Object.entries(form.attributeDrafts)
      .filter(([id, draft]) => !form.removedAttributes.includes(id) && draft.value.trim() !== "" && Number.isFinite(Number(draft.value)))
      .map(([id, draft]) => [id, draft.value]));
    updateEdit(form, (current) => ({ ...current, busy: true }));
    try {
      await updateEntity(form.original.story_id, form.original.id, name, fields, drafts);
      updateEdit(form, () => null);
    } catch (error) { console.error(error); }
    finally { updateEdit(form, (current) => ({ ...current, busy: false })); }
  };

  const onDelete = async () => {
    const form = editing;
    if (!form || form.busy) return;
    updateEdit(form, (current) => ({ ...current, busy: true }));
    try {
      await deleteEntity(form.original.story_id, form.original.id);
      updateEdit(form, () => null);
    } catch (error) { console.error(error); }
    finally { updateEdit(form, (current) => ({ ...current, busy: false })); }
  };

  const saveAttribute = async (form: EditForm, attributeId: string, draft: { value: string }) => {
    if (!draft.value.trim() || !Number.isFinite(Number(draft.value))) return;
    await setEntityAttribute(form.original.story_id, form.original.id, attributeId, Number(draft.value));
    updateEdit(form, (current) => {
      // Identity also rejects remove/re-add and same-value retyping while the set was pending.
      if (current.attributeDrafts[attributeId] !== draft) return current;
      const attributeDrafts = { ...current.attributeDrafts };
      delete attributeDrafts[attributeId];
      return { ...current, attributeDrafts };
    });
  };

  const removeAttribute = async (form: EditForm, attributeId: string) => {
    updateEdit(form, (current) => {
      const attributeDrafts = { ...current.attributeDrafts };
      delete attributeDrafts[attributeId];
      return { ...current, attributeDrafts, removedAttributes: [...current.removedAttributes, attributeId] };
    });
    try { await removeEntityAttribute(form.original.story_id, form.original.id, attributeId); }
    finally { updateEdit(form, (current) => ({ ...current, removedAttributes: current.removedAttributes.filter((id) => id !== attributeId) })); }
  };

  const addAttribute = async () => {
    const form = editing;
    const definition = availableAttributes.find((item) => item.id === form?.attributeToAdd);
    if (!form || !definition || form.busy) return;
    const draft = { value: String((definition.min + definition.max) / 2) };
    updateEdit(form, (current) => ({ ...current, attributeDrafts: { ...current.attributeDrafts, [definition.id]: draft }, attributeToAdd: "" }));
    try { await saveAttribute(form, definition.id, draft); }
    catch (error) {
      updateEdit(form, (current) => {
        if (current.attributeDrafts[definition.id] !== draft) return current;
        const attributeDrafts = { ...current.attributeDrafts };
        delete attributeDrafts[definition.id];
        return { ...current, attributeDrafts, attributeToAdd: current.attributeToAdd || definition.id };
      });
      throw error;
    }
  };

  const shown = entities.filter((entity) => entity.kind === tab);
  return (
    <div className="flex flex-col gap-2">
      <div role="group" aria-label="Entity tabs" className="flex gap-1.5">
        {(["character", "relationship"] as const).map((kind) => (
          <button key={kind} type="button" aria-label={kind === "character" ? "Characters" : "Relationships"} aria-pressed={tab === kind}
            onClick={() => {
              if (tab === kind) return;
              setTab(kind);
              setEditing(null);
              try { window.localStorage.setItem(`story-llm:entity-tab:${activeStoryId}`, kind); }
              catch { /* Keep the tab usable without persistent storage. */ }
            }}
            className={`min-w-0 flex-1 rounded border px-2 py-1 text-xs transition-colors ${tab === kind ? "border-accent text-text" : "border-border text-muted hover:text-text"}`}>
            {kind === "character" ? "Characters" : "Relationships"}
          </button>
        ))}
      </div>
      {tab === "character" && (creating ? (
        <div aria-label="New character" className="flex flex-col gap-1.5 rounded border border-border bg-bg p-2">
          <label className="flex flex-col gap-1 text-xs text-muted">Name
            <input autoFocus aria-label="Name" value={creating.name} disabled={creating.busy}
              onChange={(event) => { const name = event.target.value; setCreating((current) => current ? { ...current, name } : current); }} className={FIELD_CLASS} />
          </label>
          {renderFields(creating, (field, value) => setCreating((current) => current ? { ...current, fields: { ...current.fields, [field]: value } } : current))}
          <div className="flex gap-1.5">
            <button type="button" aria-label="Create character" onClick={submitNew} disabled={creating.busy || !creating.name.trim()}
              className="flex-1 rounded bg-accent px-2 py-1 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40">Create</button>
            <button type="button" aria-label="Cancel new character" onClick={() => setCreating(null)}
              className="rounded border border-border px-2 py-1 text-xs text-muted hover:text-text">Cancel</button>
          </div>
        </div>
      ) : (
        <button type="button" aria-label="New character" onClick={() => setCreating({ token: Symbol(), name: "", fields: characterDrafts(), busy: false })}
          className="w-full rounded border border-dashed border-border px-2 py-1.5 text-xs text-muted hover:text-text hover:border-accent transition-colors">+ New character</button>
      ))}
      {loading && <div className="text-xs text-muted py-1">Loading...</div>}
      {!loading && shown.length === 0 && <div className="text-xs text-muted py-1">{tab === "character" ? "No characters yet." : "No relationships yet."}</div>}
      <div className="flex flex-col gap-1.5">
        {shown.map((entity) => {
          const form = editing?.original.id === entity.id ? editing : null;
          if (form) return (
            <div key={entity.id} data-entity-id={entity.id} className="flex flex-col gap-1.5 rounded border border-accent bg-bg p-2">
              {entity.kind === "relationship" && <div className="text-sm text-text">{relationshipTitle(entity)}</div>}
              <label className="flex flex-col gap-1 text-xs text-muted">{entity.kind === "character" ? "Name" : "Label"}
                <input aria-label={entity.kind === "character" ? "Name" : "Label"} value={form.name} disabled={form.busy}
                  onChange={(event) => { const name = event.target.value; updateEdit(form, (current) => ({ ...current, name })); }} className={FIELD_CLASS} />
              </label>
              {entity.kind === "character" ? renderFields(form, (field, value) =>
                updateEdit(form, (current) => ({ ...current, fields: { ...current.fields, [field]: value } }))) : (
                <>
                  <label className="flex flex-col gap-1 text-xs text-muted">Direction
                    <input aria-label="Direction" readOnly value={entity.link?.direction === "both" ? "Both" : "One way"} className={`${FIELD_CLASS} text-muted`} />
                  </label>
                  <label className="flex flex-col gap-1 text-xs text-muted">Description
                    <textarea aria-label="Description" readOnly value={entity.link?.description ?? ""} rows={2} className={`${FIELD_CLASS} resize-none text-muted`} />
                  </label>
                </>
              )}
              <div className="flex flex-col gap-1.5 rounded border border-border bg-surface p-2">
                <div className="text-[11px] uppercase tracking-wide text-muted">Authoritative attributes</div>
                {attributes.filter((attribute) => !form.removedAttributes.includes(attribute.attribute_id)).map((attribute) => (
                  <div key={attribute.attribute_id} className="grid grid-cols-[minmax(0,1fr)_5rem_auto] items-center gap-1.5 text-xs">
                    <label htmlFor={`attribute-${entity.id}-${attribute.attribute_id}`} className="break-words text-text">{attribute.canonical_name}</label>
                    <input id={`attribute-${entity.id}-${attribute.attribute_id}`} aria-label={attribute.canonical_name} type="number"
                      min={attribute.min} max={attribute.max} step="0.5" disabled={form.busy}
                      value={form.attributeDrafts[attribute.attribute_id]?.value ?? String(attribute.value)}
                      onChange={(event) => { const value = event.target.value; updateEdit(form, (current) => ({ ...current, attributeDrafts: { ...current.attributeDrafts, [attribute.attribute_id]: { value } } })); }}
                      onBlur={() => { const draft = form.attributeDrafts[attribute.attribute_id]; if (draft) saveAttribute(form, attribute.attribute_id, draft).catch(console.error); }}
                      className="w-full rounded border border-border bg-bg px-1.5 py-1 text-right text-text disabled:opacity-40" />
                    <button type="button" aria-label={`Remove ${attribute.canonical_name}`} disabled={form.busy}
                      onClick={() => removeAttribute(form, attribute.attribute_id).catch(console.error)} className="text-danger disabled:opacity-40">{"\u00d7"}</button>
                  </div>
                ))}
                <div className="flex gap-1.5">
                  <select aria-label="Attribute to add" value={form.attributeToAdd} disabled={form.busy}
                    onChange={(event) => { const attributeToAdd = event.target.value; updateEdit(form, (current) => ({ ...current, attributeToAdd })); }}
                    className="min-w-0 flex-1 rounded border border-border bg-bg px-1.5 py-1 text-xs text-text">
                    <option value="">{"Add attribute\u2026"}</option>
                    {availableAttributes.map((definition) => <option key={definition.id} value={definition.id}>{definition.canonical_name}</option>)}
                  </select>
                  <button type="button" aria-label="Add attribute" onClick={() => addAttribute().catch(console.error)} disabled={form.busy || !form.attributeToAdd}
                    className="rounded border border-border px-2 py-1 text-xs text-muted disabled:opacity-40">Add</button>
                </div>
              </div>
              <div className="flex gap-1.5">
                <button type="button" aria-label={`Save ${entity.kind}`} onClick={submitEdit} disabled={form.busy || !form.name.trim()}
                  className="flex-1 rounded bg-accent px-2 py-1 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40">Save</button>
                <button type="button" aria-label={`Cancel ${entity.kind} edit`} onClick={() => setEditing(null)}
                  className="rounded border border-border px-2 py-1 text-xs text-muted hover:text-text">Cancel</button>
                <button type="button" aria-label={`Delete ${entity.kind}`} onClick={onDelete} disabled={form.busy}
                  className="rounded border border-border px-2 py-1 text-xs text-danger hover:opacity-80 disabled:opacity-40">Delete</button>
              </div>
            </div>
          );
          return (
            <div key={entity.id} data-entity-id={entity.id} className="rounded border border-border bg-bg text-text hover:border-accent transition-colors">
              <button type="button" aria-label={`Edit ${entity.kind}`} onClick={() => startEdit(entity)} className="w-full px-2 py-1.5 text-left text-sm">
                {entity.kind === "character" ? (
                  <>
                    <div className="font-medium">{entity.known_as ?? entity.name}</div>
                    {entity.location && <div className="mt-0.5 break-words text-xs text-muted">{entity.location}</div>}
                    {entity.outfit && <div className="mt-0.5 break-words text-xs text-muted">{entity.outfit}</div>}
                    {[entity.gender, entity.age, entity.role].some(Boolean) && <div className="mt-0.5 break-words text-xs text-muted">{[entity.gender, entity.age, entity.role].filter(Boolean).join(" \u00b7 ")}</div>}
                    {entity.appearance_anchor && <div className="mt-0.5 break-words text-xs text-muted">{entity.appearance_anchor}</div>}
                  </>
                ) : (
                  <>
                    <div className="break-words">{relationshipTitle(entity)}</div>
                    <div className="font-semibold">{entity.link?.label}</div>
                    {entity.link?.description && <div className="mt-0.5 break-words text-xs text-muted">{entity.link.description}</div>}
                    {(attributesByEntity[entity.id]?.length ?? 0) > 0 && <div className="mt-0.5 break-words text-xs text-muted">{attributesByEntity[entity.id].map((attribute) => `${attribute.canonical_name} ${attribute.value}`).join(" \u00b7 ")}</div>}
                  </>
                )}
              </button>
              {entity.kind === "character" && entity.known_as && (
                <div className="px-2 pb-1.5 text-xs text-muted">
                  <button type="button" aria-label={revealedIds.includes(entity.id) ? "Hide true name" : "Reveal true name"} aria-expanded={revealedIds.includes(entity.id)}
                    onClick={() => setRevealedIds((current) => current.includes(entity.id) ? current.filter((id) => id !== entity.id) : [...current, entity.id])}
                    className="hover:text-text">{revealedIds.includes(entity.id) ? "Hide" : "Reveal"}</button>
                  {revealedIds.includes(entity.id) && <div className="mt-0.5">True name: {entity.name}</div>}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
