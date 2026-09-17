import { useEffect, useState } from "react";
import { useAppStore } from "../../app/store";
import type { DiceMode } from "../../shared/types";
import { DEFAULT_DICEROLL_SETTINGS, useDicerollStore, type DicerollSettingsPatch } from "./store";

const DICE_MODES: { id: DiceMode; label: string; hint: string }[] = [
  { id: "always", label: "Always", hint: "The narrator is instructed to roll for every meaningful action." },
  { id: "classifier", label: "Classifier decides", hint: "Rolls only when the action is genuinely uncertain." },
  { id: "never", label: "Never", hint: "Pure narrative — attributes still track, but nothing gates outcomes." },
];

export function AttributesPanel() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const creatingStory = useAppStore((s) => s.creatingStory);
  const activeBranchId = useAppStore((s) => s.stories.find((story) => story.id === s.activeStoryId)?.default_branch_id ?? null);
  const settingsByStory = useDicerollStore((s) => s.settingsByStory);
  const draftSettings = useDicerollStore((s) => s.draftSettings);
  const loading = useDicerollStore((s) => s.loading);
  const loadSettings = useDicerollStore((s) => s.loadSettings);
  const saveSettings = useDicerollStore((s) => s.saveSettings);

  const [saving, setSaving] = useState(false);
  const loaded = activeStoryId ? settingsByStory[activeStoryId] : undefined;
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
      await saveSettings(activeStoryId, activeBranchId, patch);
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
    </div>
  );
}
