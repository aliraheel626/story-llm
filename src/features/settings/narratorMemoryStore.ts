import type { NarratorMemorySettings } from "../../shared/types";
import { narratorMemoryApi } from "./api";
import { createSettingsStore } from "./settingsStore";

export const useNarratorMemoryStore = createSettingsStore<
  NarratorMemorySettings,
  Parameters<typeof narratorMemoryApi.save>
>(narratorMemoryApi, "narrator memory");
