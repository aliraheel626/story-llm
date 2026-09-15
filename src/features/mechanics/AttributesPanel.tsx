import { useEffect, useState } from "react";
import { useAppStore } from "../../app/store";
import type { DiceMode } from "../../shared/types";
import { DEFAULT_MECHANICS_SETTINGS, useMechanicsStore } from "./store";

const DICE_MODES: { id: DiceMode; label: string; hint: string }[] = [
  { id: "always", label: "Always", hint: "Every Do action rolls." },
  { id: "classifier", label: "Classifier decides", hint: "Rolls only when the action is genuinely uncertain." },
  { id: "never", label: "Never", hint: "Pure narrative — attributes still track, but nothing gates outcomes." },
];

export function AttributesPanel() {
  const activeStoryId = useAppStore((s) => s.activeStoryId);
  const activeBranchId = useAppStore((s) => s.stories.find((story) => story.id === s.activeStoryId)?.default_branch_id ?? null);
  const settingsByStory = useMechanicsStore((s) => s.settingsByStory);
  const draftSettings = useMechanicsStore((s) => s.draftSettings);
  const loading = useMechanicsStore((s) => s.loading);
  const loadSettings = useMechanicsStore((s) => s.loadSettings);
  const saveSettings = useMechanicsStore((s) => s.saveSettings);

  const [saving, setSaving] = useState(false);
  const loaded = activeStoryId ? settingsByStory[activeStoryId] : undefined;
  const settings = loaded ?? draftSettings ?? DEFAULT_MECHANICS_SETTINGS;

  useEffect(() => {
    if (activeStoryId) loadSettings(activeStoryId);
  }, [activeStoryId, loadSettings]);

  if (activeStoryId && loading && !loaded) {
    return <div className="text-xs text-muted py-1">Loading...</div>;
  }

  const update = async (diceMode: DiceMode, attributesEnabled: boolean) => {
    setSaving(true);
    try {
      await saveSettings(activeStoryId, activeBranchId, diceMode, attributesEnabled);
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
          onChange={(e) => update(settings.dice_mode, e.target.checked)}
          disabled={saving}
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
                onChange={() => update(m.id, settings.attributes_enabled)}
                disabled={saving}
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
