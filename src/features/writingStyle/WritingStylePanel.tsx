import { useEffect, useState } from "react";
import { useStoryStore } from "../story/store";

export function WritingStylePanel() {
  const activeStoryId = useStoryStore((s) => s.activeStoryId);
  const note = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.authorNote : undefined);
  const loading = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.authorNoteLoading ?? false) : false);
  const saving = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.authorNoteSaving ?? false) : false);
  const loadAuthorNote = useStoryStore((s) => s.loadAuthorNote);
  const saveAuthorNote = useStoryStore((s) => s.saveAuthorNote);

  const [draft, setDraft] = useState("");
  const [savedNotice, setSavedNotice] = useState(false);

  useEffect(() => {
    if (activeStoryId) loadAuthorNote(activeStoryId);
  }, [activeStoryId, loadAuthorNote]);

  useEffect(() => {
    if (activeStoryId) setDraft(note ?? "");
  }, [activeStoryId, note]);

  if (!activeStoryId) {
    return <div className="text-xs text-muted py-1">Open a story first.</div>;
  }
  if (loading && note == null) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  const onSave = async () => {
    try {
      await saveAuthorNote(activeStoryId, draft);
      setSavedNotice(true);
      setTimeout(() => setSavedNotice(false), 2000);
    } catch (e) {
      console.error(e);
    }
  };

  return (
    <div className="flex flex-col gap-2 text-sm">
      <div>
        <label className="block text-xs text-muted mb-1">Author's note</label>
        <p className="mb-1.5 text-[11px] text-muted">
          A persistent instruction sent with every turn — tone, ongoing constraints, things to keep in mind.
        </p>
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="e.g. Keep the tone grim and understated. Never let the player die outright — injure instead."
          rows={5}
          className="w-full resize-y rounded bg-bg border border-border px-2 py-1.5 text-sm text-text placeholder:text-muted focus:outline-none focus:border-accent"
        />
      </div>
      <button
        onClick={onSave}
        disabled={saving}
        className="rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
