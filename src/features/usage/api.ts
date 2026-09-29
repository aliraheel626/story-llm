import { invoke } from "@tauri-apps/api/core";
import type { StoryUsage } from "../../shared/types";

export const usageApi = {
  get: (storyId: string) => invoke<StoryUsage>("get_story_usage", { storyId }),
};
