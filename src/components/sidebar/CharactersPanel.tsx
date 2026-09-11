import { useEffect, useState } from "react";
import { useAppStore } from "../../store/appStore";
import { useCharacterStore } from "../../store/characterStore";
import type { Entity } from "../../lib/types";

export function CharactersPanel() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
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

  const characters = activeStoryId ? (entitiesByStory[activeStoryId] ?? []) : [];

  useEffect(() => {
    if (activeStoryId) loadCharacters(activeStoryId);
  }, [activeStoryId, loadCharacters]);

  if (!activeStoryId) {
    return <div className="text-xs text-muted py-1">Open a story first.</div>;
  }

  const submitNew = async () => {
    const trimmed = name.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    try {
      await createCharacter(activeStoryId, trimmed, anchor.trim() || undefined);
      setName("");
      setAnchor("");
      setCreating(false);
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
  };

  const startEdit = (entity: Entity) => {
    setEditingId(entity.id);
    setEditName(entity.name);
    setEditAnchor(entity.appearance_anchor ?? "");
  };

  const submitEdit = async () => {
    if (!editingId || busy) return;
    const trimmed = editName.trim();
    if (!trimmed) return;
    setBusy(true);
    try {
      await updateCharacter(activeStoryId, editingId, trimmed, editAnchor.trim() || undefined);
      setEditingId(null);
    } catch (e) {
      console.error(e);
    } finally {
      setBusy(false);
    }
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
                  onClick={() => deleteCharacter(activeStoryId, entity.id)}
                  className="rounded border border-border px-2 py-1 text-xs text-danger hover:opacity-80"
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
