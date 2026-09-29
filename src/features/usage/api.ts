import { invoke } from "@tauri-apps/api/core";
import type { StoryCostBreakdown, StoryUsage } from "../../shared/types";

export const usageApi = {
  get: (storyId: string) => invoke<StoryUsage>("get_story_usage", { storyId }),
  breakdown: (storyId: string) => invoke<StoryCostBreakdown>("get_story_usage_breakdown", { storyId }),
};
