import { invoke } from "@tauri-apps/api/core";
import type { DiceMode, DicerollSettings, ReasoningEffort } from "../../shared/types";

export const dicerollApi = {
  getSettings: (storyId: string) => invoke<DicerollSettings>("get_story_diceroll_settings", { storyId }),
  saveSettings: (storyId: string, branchId: string, diceMode: DiceMode, attributesEnabled: boolean, reasoningEffort: ReasoningEffort | null) =>
    invoke<void>("save_story_diceroll_settings", { storyId, branchId, diceMode, attributesEnabled, reasoningEffort }),
};
