import type { ImageModelSettings } from "../../shared/types";
import { imageModelApi } from "./api";
import { createSettingsStore } from "./settingsStore";

export const useImageModelStore = createSettingsStore<ImageModelSettings, Parameters<typeof imageModelApi.save>>(
  imageModelApi,
  "image model",
);
