import { useEffect, useState } from "react";
import { REASONING_EFFORT_OPTIONS, type DiceMode, type ReasoningEffort } from "../../shared/types";
import { DEFAULT_DICEROLL_SETTINGS, useStoryStore, type DicerollSettingsPatch } from "../story/store";

const DICE_MODES: { id: DiceMode; label: string; hint: string }[] = [
  { id: "always", label: "Always", hint: "The narrator is instructed to roll for every meaningful action." },
  { id: "classifier", label: "Classifier decides", hint: "Rolls only when the action is genuinely uncertain." },
  { id: "never", label: "Never", hint: "Pure narrative — attributes still track, but nothing gates outcomes." },
];

export function AttributesPanel() {
  const activeStoryId = useStoryStore((s) => s.activeStoryId);
  const creatingStory = useStoryStore((s) => s.creatingStory);
  const loaded = useStoryStore((s) => activeStoryId ? s.bundles[activeStoryId]?.diceSettings : undefined);
  const draftSettings = useStoryStore((s) => s.draftDiceSettings);
  const loading = useStoryStore((s) => activeStoryId ? (s.bundles[activeStoryId]?.diceSettingsLoading ?? false) : false);
  const loadSettings = useStoryStore((s) => s.loadDiceSettings);
  const saveSettings = useStoryStore((s) => s.saveDiceSettings);

  const [saving, setSaving] = useState(false);
  const settings = loaded ?? draftSettings ?? DEFAULT_DICEROLL_SETTINGS;

  useEffect(() => {
    if (activeStoryId) loadSettings(activeStoryId);
  }, [activeStoryId, loadSettings]);

  if (activeStoryId && loading && !loaded) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  const update = async (patch: DicerollSettingsPatch) => {
    setSaving(true);
    try {
      await saveSettings(activeStoryId, patch);
    } catch (e) {
      console.error(e);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="flex flex-col gap-3 text-sm">
      <label className="flex items-start gap-2">
        <input
          type="checkbox"
          checked={settings.attributes_enabled}
          onChange={(e) => update({ attributes_enabled: e.target.checked })}
          disabled={saving || creatingStory}
          className="mt-0.5 accent-accent"
        />
        <span>
          <span className="text-text">Attributes</span>
          <span className="block text-xs text-muted">
            Entities track stats (Accuracy, Trust, …) that shape both dice odds and narration tone. Off removes the
            narrator's dice-roll and entity-tracking tools entirely.
          </span>
        </span>
      </label>

      <div className={settings.attributes_enabled ? "" : "opacity-40 pointer-events-none"}>
        <div className="mb-1 text-xs text-muted">Dice rolls</div>
        <div className="flex flex-col gap-1">
          {DICE_MODES.map((m) => (
            <label key={m.id} className="flex items-start gap-2 rounded border border-border bg-bg px-2 py-1.5 has-[:checked]:border-accent">
              <input
                type="radio"
                name="dice-mode"
                checked={settings.dice_mode === m.id}
                onChange={() => update({ dice_mode: m.id })}
                disabled={saving || creatingStory}
                className="mt-0.5 accent-accent"
              />
              <span>
                <span className="text-text">{m.label}</span>
                <span className="block text-xs text-muted">{m.hint}</span>
              </span>
            </label>
          ))}
        </div>
      </div>

      <label className="flex items-center justify-between gap-2 text-xs text-muted">
        <span>Reasoning effort</span>
        <select
          value={settings.reasoning_effort ?? ""}
          onChange={(event) => update({ reasoning_effort: (event.target.value || null) as ReasoningEffort | null })}
          disabled={saving || creatingStory}
          className="rounded border border-border bg-bg px-1.5 py-1 text-xs text-text focus:border-accent focus:outline-none disabled:opacity-60"
        >
          {REASONING_EFFORT_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>{option.label}</option>
          ))}
        </select>
      </label>
    </div>
  );
}
