import { useState } from "react";
import type { EntityContextMode } from "../../shared/types";
import { useNarratorMemoryStore } from "./narratorMemoryStore";
import { useSettingsForm } from "./useSettingsForm";

export function NarratorMemoryPanel() {
  const [toolCallPersistence, setToolCallPersistence] = useState(true);
  const [entityContextMode, setEntityContextMode] = useState<EntityContextMode>("all");
  const { settings, loading, saving, savedNotice, save } = useSettingsForm(useNarratorMemoryStore, (next) => {
    setToolCallPersistence(next.tool_call_persistence);
    setEntityContextMode(next.entity_context_mode);
  });

  const onSave = async () => {
    await save(toolCallPersistence, entityContextMode);
  };

  if (loading && !settings) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  return (
    <div className="flex flex-col gap-2 text-sm">
      <label className="flex items-start gap-2 text-xs text-muted">
        <input
          type="checkbox"
          checked={toolCallPersistence}
          onChange={(e) => setToolCallPersistence(e.target.checked)}
          className="mt-0.5 accent-accent"
        />
        <span>
          Persist tool lookups to the timeline
          <span className="mt-0.5 block text-[11px]">Keeps entity lookups available to later turns and summaries.</span>
        </span>
      </label>

      <div>
        <label className="block text-xs text-muted mb-1">Entity context</label>
        <select
          value={entityContextMode}
          onChange={(e) => setEntityContextMode(e.target.value as EntityContextMode)}
          className="w-full rounded bg-bg border border-border px-2 py-1.5 text-xs text-text focus:outline-none focus:border-accent"
        >
          <option value="all">Full entity dump every turn</option>
          <option value="scoped">Scoped to recently active entities</option>
          <option value="none">Off — no entity context sent</option>
        </select>
      </div>

      <button
        onClick={onSave}
        disabled={saving}
        className="mt-1 rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
