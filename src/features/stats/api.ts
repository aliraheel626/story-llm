import { invoke } from "@tauri-apps/api/core";
import type { StoryStats } from "../../shared/types";

export const statsApi = {
  get: (storyId: string) => invoke<StoryStats>("get_story_stats", { storyId }),
};
