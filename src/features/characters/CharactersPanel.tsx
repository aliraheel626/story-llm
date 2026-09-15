import { useEffect, useState } from "react";
import { useAppStore } from "../../app/store";
import type { AttributeRegistryEntry, Entity, EntityAttributeValue } from "../../shared/types";
import { charactersApi } from "./api";
import { useCharacterStore } from "./store";

export function CharactersPanel() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const activeBranchId = useAppStore((s) => s.stories.find((story) => story.id === s.activeStoryId)?.default_branch_id ?? null);
  const entitiesByStory = useCharacterStore((s) => s.entitiesByStory);
  const loading = useCharacterStore((s) => s.loading);
  const loadCharacters = useCharacterStore((s) => s.loadCharacters);
  const createCharacter = useCharacterStore((s) => s.createCharacter);
  const updateCharacter = useCharacterStore((s) => s.updateCharacter);
  const deleteCharacter = useCharacterStore((s) => s.deleteCharacter);

  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [anchor, setAnchor] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editAnchor, setEditAnchor] = useState("");
  const [busy, setBusy] = useState(false);
  const [attributes, setAttributes] = useState<EntityAttributeValue[]>([]);
  const [registry, setRegistry] = useState<AttributeRegistryEntry[]>([]);
  const [attributeDrafts, setAttributeDrafts] = useState<Record<string, string>>({});
  const [attributeToAdd, setAttributeToAdd] = useState("");

  const characters = activeStoryId ? (entitiesByStory[activeStoryId] ?? []) : [];

  useEffect(() => {
    if (activeStoryId && activeBranchId) loadCharacters(activeStoryId, activeBranchId);
  }, [activeStoryId, activeBranchId, loadCharacters]);

  useEffect(() => { charactersApi.listRegistry().then(setRegistry).catch(console.error); }, []);

  if (!activeStoryId || !activeBranchId) {
    return <div className="text-xs text-muted py-1">Open a story first.</div>;
  }

  const submitNew = async () => {
    const trimmed = name.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    try {
      await createCharacter(activeStoryId, activeBranchId, trimmed, anchor.trim() || undefined);
      setName("");
      setAnchor("");
      setCreating(false);
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  const startEdit = async (entity: Entity) => {
    setEditingId(entity.id);
    setEditName(entity.name);
    setEditAnchor(entity.appearance_anchor ?? "");
    try {
      const values = await charactersApi.listAttributes(activeBranchId, entity.id);
      setAttributes(values);
      setAttributeDrafts(Object.fromEntries(values.map((value) => [value.attribute_id, String(value.value)])));
      setAttributeToAdd("");
    } catch (error) { console.error(error); }
  };

  const submitEdit = async () => {
    if (!editingId || busy) return;
    const trimmed = editName.trim();
    if (!trimmed) return;
    setBusy(true);
    try {
      for (const attribute of attributes) {
        const value = Number(attributeDrafts[attribute.attribute_id] ?? attribute.value);
        if (Number.isFinite(value) && value !== attribute.value) {
          await charactersApi.setAttribute(activeBranchId, editingId, attribute.attribute_id, value);
        }
      }
      await updateCharacter(activeStoryId, activeBranchId, editingId, trimmed, editAnchor.trim() || undefined);
      setEditingId(null);
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  // Only closes the edit card on success, so a failed delete leaves the row
  // open to retry.
  const onDelete = async () => {
    if (busy || !editingId) return;
    setBusy(true);
    try {
      await deleteCharacter(activeStoryId, activeBranchId, editingId);
      setEditingId(null);
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  const saveAttribute = async (attributeId: string) => {
    if (!editingId) return;
    const value = Number(attributeDrafts[attributeId]);
    if (!Number.isFinite(value)) return;
    const saved = await charactersApi.setAttribute(activeBranchId, editingId, attributeId, value);
    setAttributes((current) => current.some((item) => item.attribute_id === attributeId)
      ? current.map((item) => item.attribute_id === attributeId ? saved : item)
      : [...current, saved].sort((a, b) => a.canonical_name.localeCompare(b.canonical_name)));
  };

  const removeAttribute = async (attributeId: string) => {
    if (!editingId) return;
    await charactersApi.removeAttribute(activeBranchId, editingId, attributeId);
    setAttributes((current) => current.filter((item) => item.attribute_id !== attributeId));
  };

  const addAttribute = async () => {
    const definition = registry.find((item) => item.id === attributeToAdd);
    if (!definition) return;
    const midpoint = (definition.min + definition.max) / 2;
    setAttributeDrafts((current) => ({ ...current, [definition.id]: String(midpoint) }));
    if (!editingId) return;
    const saved = await charactersApi.setAttribute(activeBranchId, editingId, definition.id, midpoint);
    setAttributes((current) => [...current, saved].sort((a, b) => a.canonical_name.localeCompare(b.canonical_name)));
    setAttributeToAdd("");
  };

  return (
    <div className="flex flex-col gap-2">
      {creating ? (
        <div className="flex flex-col gap-1.5 rounded border border-border bg-bg p-2">
          <input
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Name"
            className="w-full rounded bg-surface border border-border px-2 py-1 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
          />
          <textarea
            value={anchor}
            onChange={(e) => setAnchor(e.target.value)}
            placeholder="Appearance (used to keep generated images consistent)..."
            rows={3}
            className="w-full resize-none rounded bg-surface border border-border px-2 py-1 text-xs text-text placeholder:text-muted focus:outline-none focus:border-accent"
          />
          <div className="flex gap-1.5">
            <button
              onClick={submitNew}
              disabled={busy || !name.trim()}
              className="flex-1 rounded bg-accent px-2 py-1 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40"
            >
              Create
            </button>
            <button onClick={() => setCreating(false)} className="rounded border border-border px-2 py-1 text-xs text-muted hover:text-text">
              Cancel
            </button>
          </div>
        </div>
      ) : (
        <button
          onClick={() => setCreating(true)}
          className="w-full rounded border border-dashed border-border px-2 py-1.5 text-xs text-muted hover:text-text hover:border-accent transition-colors"
        >
          + New character
        </button>
      )}

      {loading && <div className="text-xs text-muted py-1">Loading...</div>}
      {!loading && characters.length === 0 && <div className="text-xs text-muted py-1">No characters yet.</div>}

      <div className="flex flex-col gap-1.5">
        {characters.map((entity) =>
          editingId === entity.id ? (
            <div key={entity.id} className="flex flex-col gap-1.5 rounded border border-accent bg-bg p-2">
              <input
                value={editName}
                onChange={(e) => setEditName(e.target.value)}
                className="w-full rounded bg-surface border border-border px-2 py-1 text-sm text-text focus:outline-none focus:border-accent"
              />
              <textarea
                value={editAnchor}
                onChange={(e) => setEditAnchor(e.target.value)}
                rows={3}
                className="w-full resize-none rounded bg-surface border border-border px-2 py-1 text-xs text-text focus:outline-none focus:border-accent"
              />
              <div className="flex flex-col gap-1.5 rounded border border-border bg-surface p-2">
                <div className="text-[11px] uppercase tracking-wide text-muted">Authoritative attributes</div>
                {attributes.map((attribute) => (
                  <div key={attribute.attribute_id} className="grid grid-cols-[1fr_5rem_auto] items-center gap-1.5 text-xs">
                    <label className="text-text">{attribute.canonical_name}</label>
                    <input type="number" min={attribute.min} max={attribute.max} step="0.5"
                      value={attributeDrafts[attribute.attribute_id] ?? String(attribute.value)}
                      onChange={(event) => setAttributeDrafts((current) => ({ ...current, [attribute.attribute_id]: event.target.value }))}
                      onBlur={() => saveAttribute(attribute.attribute_id).catch(console.error)}
                      className="w-full rounded border border-border bg-bg px-1.5 py-1 text-right text-text" />
                    <button onClick={() => removeAttribute(attribute.attribute_id).catch(console.error)} className="text-danger" title="Remove attribute">×</button>
                  </div>
                ))}
                <div className="flex gap-1.5">
                  <select value={attributeToAdd} onChange={(event) => setAttributeToAdd(event.target.value)} className="min-w-0 flex-1 rounded border border-border bg-bg px-1.5 py-1 text-xs text-text">
                    <option value="">Add attribute…</option>
                    {registry.filter((definition) => !attributes.some((item) => item.attribute_id === definition.id) && JSON.parse(definition.entity_kinds_json).includes("character"))
                      .map((definition) => <option key={definition.id} value={definition.id}>{definition.canonical_name}</option>)}
                  </select>
                  <button onClick={() => addAttribute().catch(console.error)} disabled={!attributeToAdd} className="rounded border border-border px-2 py-1 text-xs text-muted disabled:opacity-40">Add</button>
                </div>
              </div>
              <div className="flex gap-1.5">
                <button
                  onClick={submitEdit}
                  disabled={busy || !editName.trim()}
                  className="flex-1 rounded bg-accent px-2 py-1 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40"
                >
                  Save
                </button>
                <button onClick={() => setEditingId(null)} className="rounded border border-border px-2 py-1 text-xs text-muted hover:text-text">
                  Cancel
                </button>
                <button
                  onClick={onDelete}
                  disabled={busy}
                  className="rounded border border-border px-2 py-1 text-xs text-danger hover:opacity-80 disabled:opacity-40"
                >
                  Delete
                </button>
              </div>
            </div>
          ) : (
            <button
              key={entity.id}
              onClick={() => startEdit(entity)}
              className="w-full rounded border border-border bg-bg px-2 py-1.5 text-left text-sm text-text hover:border-accent transition-colors"
            >
              <div className="font-medium">{entity.name}</div>
              {entity.appearance_anchor && <div className="mt-0.5 truncate text-xs text-muted">{entity.appearance_anchor}</div>}
            </button>
          ),
        )}
      </div>
    </div>
  );
}
