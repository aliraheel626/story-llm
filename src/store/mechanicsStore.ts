import { create } from "zustand";
import { commands } from "../lib/commands";
import type { DiceMode, MechanicsSettings } from "../lib/types";

interface MechanicsState {
  settingsByStory: Record<string, MechanicsSettings>;
  loading: boolean;

  loadSettings: (storyId: string) => Promise<void>;
  saveSettings: (storyId: string, diceMode: DiceMode, attributesEnabled: boolean) => Promise<void>;
}

export const useMechanicsStore = create<MechanicsState>((set) => ({
  settingsByStory: {},
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

  saveSettings: async (storyId: string, diceMode: DiceMode, attributesEnabled: boolean) => {
    await commands.saveStoryMechanicsSettings(storyId, diceMode, attributesEnabled);
    set((s) => ({ settingsByStory: { ...s.settingsByStory, [storyId]: { dice_mode: diceMode, attributes_enabled: attributesEnabled } } }));
  },
}));
