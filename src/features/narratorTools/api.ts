import { invoke } from "@tauri-apps/api/core";
import type { NarratorToolSettings, ReasoningEffort } from "../../shared/types";

export const narratorToolsApi = {
  get: (storyId: string) => invoke<NarratorToolSettings>("get_story_narrator_tools", { storyId }),
  save: (storyId: string, tools: NarratorToolSettings) => invoke<void>("save_story_narrator_tools", { storyId, tools }),
  getReasoningEffort: async (storyId: string): Promise<ReasoningEffort | null> =>
    (await invoke<ReasoningEffort | "">("get_story_reasoning_effort", { storyId })) || null,
  saveReasoningEffort: (storyId: string, reasoningEffort: ReasoningEffort | null) =>
    invoke<void>("save_story_reasoning_effort", { storyId, reasoningEffort: reasoningEffort ?? "" }),
};
