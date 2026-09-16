import { create } from "zustand";
import type { DicerollSettings } from "../../shared/types";
import { dicerollApi } from "./api";

/** Backend defaults for a story whose `settings_json` has no dice-roll keys —
 *  see `dicerolls::commands::get_story_diceroll_settings`. */
export const DEFAULT_DICEROLL_SETTINGS: DicerollSettings = { dice_mode: "classifier", attributes_enabled: true, reasoning_effort: null };

/** A partial settings change. Callers pass only the field they touched so a
 *  save can't silently reset the others. */
export type DicerollSettingsPatch = Partial<DicerollSettings>;

interface DicerollState {
  settingsByStory: Record<string, DicerollSettings>;
  /** Settings chosen while composing a not-yet-persisted draft story; passed
   *  to `create_story` on first submit so they land atomically with the story. */
  draftSettings: DicerollSettings | null;
  loading: boolean;

  loadSettings: (storyId: string) => Promise<void>;
  saveSettings: (storyId: string | null, branchId: string | null, patch: DicerollSettingsPatch) => Promise<void>;
  resetDraftSettings: () => void;
  promoteDraftSettings: (storyId: string) => void;
}

export const useDicerollStore = create<DicerollState>((set, get) => ({
  settingsByStory: {},
  draftSettings: null,
  loading: false,

  loadSettings: async (storyId: string) => {
    set({ loading: true });
    try {
      const settings = await dicerollApi.getSettings(storyId);
      set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: settings }, loading: false }));
    } catch (e) {
      console.error("failed to load dice-roll settings", e);
      set({ loading: false });
    }
  },

  saveSettings: async (storyId: string | null, branchId: string | null, patch: DicerollSettingsPatch) => {
    const current = (storyId ? get().settingsByStory[storyId] : get().draftSettings) ?? DEFAULT_DICEROLL_SETTINGS;
    const next: DicerollSettings = { ...current, ...patch };
    if (!storyId || !branchId) {
      set({ draftSettings: next });
      return;
    }
    await dicerollApi.saveSettings(storyId, branchId, next.dice_mode, next.attributes_enabled, next.reasoning_effort);
    set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: next } }));
  },

  resetDraftSettings: () => set({ draftSettings: null }),

  // Seeds the new story's cache entry from the draft so the shared panels
  // don't flash backend defaults before their first fetch lands; persistence
  // already happened inside `create_story`.
  promoteDraftSettings: (storyId: string) => {
    const draft = get().draftSettings;
    if (!draft) return;
    set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: draft }, draftSettings: null }));
  },
}));
