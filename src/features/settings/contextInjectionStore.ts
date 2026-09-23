import { create } from "zustand";
import type { ContextInjectionSettings } from "../../shared/types";
import { contextInjectionApi } from "./api";
import { createSettingsStore } from "./settingsStore";

export const useContextInjectionStore = createSettingsStore<
  ContextInjectionSettings,
  Parameters<typeof contextInjectionApi.save>
>(contextInjectionApi, "context injection");

export const useDiceRollsContextDraft = create<{
  enabled: boolean;
  source: ContextInjectionSettings | null;
  setEnabled: (enabled: boolean) => void;
  sync: (settings: ContextInjectionSettings) => void;
}>((set) => ({
  enabled: true,
  source: null,
  setEnabled: (enabled) => set({ enabled }),
  sync: (settings) => set((state) => state.source === settings
    ? state
    : { source: settings, enabled: settings.dice_rolls_in_context }),
}));
