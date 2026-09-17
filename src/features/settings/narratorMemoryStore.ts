import { create } from "zustand";
import type { NarratorMemorySettings, NarratorPreambleMode } from "../../shared/types";
import { narratorMemoryApi } from "./api";

interface NarratorMemoryState {
  settings: NarratorMemorySettings | null;
  loading: boolean;
  saving: boolean;
  load: () => Promise<void>;
  save: (toolCallPersistence: boolean, preambleMode: NarratorPreambleMode) => Promise<void>;
}

export const useNarratorMemoryStore = create<NarratorMemoryState>((set) => ({
  settings: null,
  loading: false,
  saving: false,
  load: async () => {
    set({ loading: true });
    try {
      const settings = await narratorMemoryApi.get();
      set({ settings, loading: false });
    } catch (e) {
      console.error("failed to load narrator memory settings", e);
      set({ loading: false });
    }
  },
  save: async (toolCallPersistence, preambleMode) => {
    set({ saving: true });
    try {
      await narratorMemoryApi.save(toolCallPersistence, preambleMode);
      const settings = await narratorMemoryApi.get();
      set({ settings, saving: false });
    } catch (e) {
      set({ saving: false });
      throw e;
    }
  },
}));
