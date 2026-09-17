import type { TextModelSettings } from "../../shared/types";
import { textModelApi } from "./api";
import { createSettingsStore } from "./settingsStore";

export const useTextModelStore = createSettingsStore<TextModelSettings, Parameters<typeof textModelApi.save>>(
  textModelApi,
  "text model",
);
