import { create } from "zustand";
import type { ImageModelSettings } from "../../shared/types";
import { imageModelApi } from "./api";

interface ImageModelState {
  settings: ImageModelSettings | null;
  loading: boolean;
  saving: boolean;
  load: () => Promise<void>;
  save: (model: string, enabled: boolean, style: string, narratorImages: boolean) => Promise<void>;
}

export const useImageModelStore = create<ImageModelState>((set) => ({
  settings: null,
  loading: false,
  saving: false,
  load: async () => {
    set({ loading: true });
    try {
      const settings = await imageModelApi.get();
      set({ settings, loading: false });
    } catch (e) {
      console.error("failed to load image model settings", e);
      set({ loading: false });
    }
  },
  save: async (model, enabled, style, narratorImages) => {
    set({ saving: true });
    try {
      await imageModelApi.save(model, enabled, style, narratorImages);
      const settings = await imageModelApi.get();
      set({ settings, saving: false });
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
