import { create } from "zustand";
import type { TextModelSettings } from "../../shared/types";
import { textModelApi } from "./api";

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
      const settings = await textModelApi.get();
      set({ settings, loading: false });
    } catch (e) {
      console.error("failed to load text model settings", e);
      set({ loading: false });
    }
  },
  save: async (provider, model, apiKey) => {
    set({ saving: true });
    try {
      await textModelApi.save(provider, model, apiKey);
      const settings = await textModelApi.get();
      set({ settings, saving: false });
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
