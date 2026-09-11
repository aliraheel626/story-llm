import { create } from "zustand";
import { commands } from "../lib/commands";
import type { DiceMode, MechanicsSettings } from "../lib/types";

/** Backend defaults for a story whose `settings_json` has no mechanics keys —
 *  see `commands::mechanics::get_story_mechanics_settings`. */
export const DEFAULT_MECHANICS_SETTINGS: MechanicsSettings = { dice_mode: "classifier", attributes_enabled: true };

interface MechanicsState {
  settingsByStory: Record<string, MechanicsSettings>;
  /** Settings chosen while composing a not-yet-persisted draft story; passed
   *  to `create_story` on first submit so they land atomically with the story. */
  draftSettings: MechanicsSettings | null;
  loading: boolean;

  loadSettings: (storyId: string) => Promise<void>;
  saveSettings: (storyId: string | null, diceMode: DiceMode, attributesEnabled: boolean) => Promise<void>;
  resetDraftSettings: () => void;
  promoteDraftSettings: (storyId: string) => void;
}

export const useMechanicsStore = create<MechanicsState>((set, get) => ({
  settingsByStory: {},
  draftSettings: null,
  loading: false,

  loadSettings: async (storyId: string) => {
    set({ loading: true });
    try {
      const settings = await commands.getStoryMechanicsSettings(storyId);
      set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: settings }, loading: false }));
    } catch (e) {
      console.error("failed to load mechanics settings", e);
      set({ loading: false });
    }
  },

  saveSettings: async (storyId: string | null, diceMode: DiceMode, attributesEnabled: boolean) => {
    if (!storyId) {
      set({ draftSettings: { dice_mode: diceMode, attributes_enabled: attributesEnabled } });
      return;
    }
    await commands.saveStoryMechanicsSettings(storyId, diceMode, attributesEnabled);
    set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: { dice_mode: diceMode, attributes_enabled: attributesEnabled } } }));
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
