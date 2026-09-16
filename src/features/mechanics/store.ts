import { create } from "zustand";
import type { MechanicsSettings } from "../../shared/types";
import { mechanicsApi } from "./api";

/** Backend defaults for a story whose `settings_json` has no mechanics keys —
 *  see `commands::mechanics::get_story_mechanics_settings`. */
export const DEFAULT_MECHANICS_SETTINGS: MechanicsSettings = { dice_mode: "classifier", attributes_enabled: true, reasoning_effort: null };

/** A partial settings change. Callers pass only the field they touched so a
 *  save can't silently reset the others. */
export type MechanicsPatch = Partial<MechanicsSettings>;

interface MechanicsState {
  settingsByStory: Record<string, MechanicsSettings>;
  /** Settings chosen while composing a not-yet-persisted draft story; passed
   *  to `create_story` on first submit so they land atomically with the story. */
  draftSettings: MechanicsSettings | null;
  loading: boolean;

  loadSettings: (storyId: string) => Promise<void>;
  saveSettings: (storyId: string | null, branchId: string | null, patch: MechanicsPatch) => Promise<void>;
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
      const settings = await mechanicsApi.getSettings(storyId);
      set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: settings }, loading: false }));
    } catch (e) {
      console.error("failed to load mechanics settings", e);
      set({ loading: false });
    }
  },

  saveSettings: async (storyId: string | null, branchId: string | null, patch: MechanicsPatch) => {
    const current = (storyId ? get().settingsByStory[storyId] : get().draftSettings) ?? DEFAULT_MECHANICS_SETTINGS;
    const next: MechanicsSettings = { ...current, ...patch };
    if (!storyId || !branchId) {
      set({ draftSettings: next });
      return;
    }
    await mechanicsApi.saveSettings(storyId, branchId, next.dice_mode, next.attributes_enabled, next.reasoning_effort);
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
