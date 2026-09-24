import type { ContextInjectionSettings } from "../../shared/types";
import { contextInjectionApi } from "./api";
import { createSettingsStore } from "./settingsStore";

export const useContextInjectionStore = createSettingsStore<
  ContextInjectionSettings,
  Parameters<typeof contextInjectionApi.save>
>(contextInjectionApi, "context injection");
