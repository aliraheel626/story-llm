import { invoke } from "@tauri-apps/api/core";
import type { DiceMode, DicerollSettings, ReasoningEffort, RollDetail } from "../../shared/types";

export const dicerollApi = {
  getSettings: (storyId: string) => invoke<DicerollSettings>("get_story_diceroll_settings", { storyId }),
  saveSettings: (storyId: string, diceMode: DiceMode, attributesEnabled: boolean, reasoningEffort: ReasoningEffort | null) =>
    invoke<void>("save_story_diceroll_settings", { storyId, diceMode, attributesEnabled, reasoningEffort }),
  listRollDetailsForEntry: (storyId: string, entryId: string) => invoke<RollDetail[]>("list_roll_details_for_entry", { storyId, entryId }),
};
