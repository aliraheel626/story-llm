import { create } from "zustand";
import { commands } from "../lib/commands";
import type { TextModelSettings } from "../lib/types";

interface TextModelState {
  settings: TextModelSettings | null;
  loading: boolean;
  saving: boolean;
  load: () => Promise<void>;
  save: (provider: string, model: string, apiKey?: string) => Promise<void>;
}

export const useTextModelStore = create<TextModelState>((set) => ({
  settings: null,
  loading: false,
  saving: false,
  load: async () => {
    set({ loading: true });
    try {
      const settings = await commands.getTextModelSettings();
      set({ settings, loading: false });
    } catch (e) {
      console.error("failed to load text model settings", e);
      set({ loading: false });
    }
  },
  save: async (provider, model, apiKey) => {
    set({ saving: true });
    try {
      await commands.saveTextModelSettings(provider, model, apiKey);
      const settings = await commands.getTextModelSettings();
      set({ settings, saving: false });
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
