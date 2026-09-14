import { create } from "zustand";
import { commands } from "../lib/commands";
import type { ImageModelSettings } from "../lib/types";

interface ImageModelState {
  settings: ImageModelSettings | null;
  loading: boolean;
  saving: boolean;
  load: () => Promise<void>;
  save: (model: string, enabled: boolean, style: string) => Promise<void>;
}

export const useImageModelStore = create<ImageModelState>((set) => ({
  settings: null,
  loading: false,
  saving: false,
  load: async () => {
    set({ loading: true });
    try {
      const settings = await commands.getImageModelSettings();
      set({ settings, loading: false });
    } catch (e) {
      console.error("failed to load image model settings", e);
      set({ loading: false });
    }
  },
  save: async (model, enabled, style) => {
    set({ saving: true });
    try {
      await commands.saveImageModelSettings(model, enabled, style);
      const settings = await commands.getImageModelSettings();
      set({ settings, saving: false });
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
