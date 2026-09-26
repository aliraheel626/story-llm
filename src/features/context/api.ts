import { invoke } from "@tauri-apps/api/core";
import type { ContextItem, ContextPreview, InjectionSettings } from "../../shared/types";

export const contextApi = {
  get: (storyId: string) => invoke<ContextItem[]>("get_story_context_settings", { storyId }),
  save: (storyId: string, include: Record<string, boolean>) =>
    invoke<ContextItem[]>("save_story_context_settings", { storyId, include }),
  getInjection: (storyId: string) => invoke<InjectionSettings>("get_story_injection_settings", { storyId }),
  saveInjection: (storyId: string, settings: InjectionSettings) =>
    invoke<void>("save_story_injection_settings", { storyId, settings }),
  preview: (storyId: string) => invoke<ContextPreview>("preview_story_context", { storyId }),
};
