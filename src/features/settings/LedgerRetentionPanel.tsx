import { useState } from "react";
import { useContextInjectionStore, useDiceRollsContextDraft } from "./contextInjectionStore";
import { useLedgerRetentionStore } from "./ledgerRetentionStore";
import { useSettingsForm } from "./useSettingsForm";

export function LedgerRetentionPanel() {
  const [toolCallPersistence, setToolCallPersistence] = useState(true);
  const diceRollsInContext = useDiceRollsContextDraft((state) => state.enabled);
  const setDiceRollsInContext = useDiceRollsContextDraft((state) => state.setEnabled);
  const {
    settings: contextSettings,
    saving: savingDice,
    savedNotice: diceSavedNotice,
    save: saveDice,
  } = useSettingsForm(useContextInjectionStore, (next) => {
    useDiceRollsContextDraft.getState().sync(next);
  }, true);
  const { settings, loading, saving, savedNotice, save } = useSettingsForm(useLedgerRetentionStore, (next) => {
    setToolCallPersistence(next.tool_call_persistence);
  });

  if (loading && !settings) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  return (
    <div className="flex flex-col gap-2 text-sm">
      <label className="flex items-start gap-2 text-xs text-muted">
        <input
          type="checkbox"
          checked={toolCallPersistence}
          onChange={(event) => setToolCallPersistence(event.target.checked)}
          className="mt-0.5 accent-accent"
        />
        <span>
          Persist tool lookups to the ledger
          <span className="mt-0.5 block text-[11px]">Keeps entity lookups available to later turns and summaries.</span>
          <span className="mt-0.5 block text-[11px]">
            This must be on when Entity Context is Off for ledger lookups to remain available.
          </span>
        </span>
      </label>

      <label className="flex items-start gap-2 text-xs text-muted">
        <input
          type="checkbox"
          checked={diceRollsInContext}
          onChange={(event) => setDiceRollsInContext(event.target.checked)}
          disabled={!contextSettings || savingDice}
          className="mt-0.5 accent-accent"
        />
        <span>
          Include dice rolls in context
          <span className="mt-0.5 block text-[11px]">Replays saved roll outcomes to the narrator on later turns; roll records stay on the ledger when off.</span>
        </span>
      </label>

      <button
        onClick={() => {
          const current = useContextInjectionStore.getState().settings;
          if (current) saveDice(current.entity_context_mode, diceRollsInContext);
        }}
        disabled={!contextSettings || savingDice}
        className="rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {savingDice ? "Saving..." : diceSavedNotice ? "Dice setting saved" : "Save dice setting"}
      </button>

      <button
        onClick={() => save(toolCallPersistence)}
        disabled={saving}
        className="mt-1 rounded bg-accent px-2 py-1.5 text-xs font-medium text-bg hover:bg-accent-hover disabled:opacity-40 transition-colors"
      >
        {saving ? "Saving..." : savedNotice ? "Saved" : "Save"}
      </button>
    </div>
  );
}
