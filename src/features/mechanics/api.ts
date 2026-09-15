import { invoke } from "@tauri-apps/api/core";
import type { DiceMode, MechanicsSettings } from "../../shared/types";

export const mechanicsApi = {
  getSettings: (storyId: string) => invoke<MechanicsSettings>("get_story_mechanics_settings", { storyId }),
  saveSettings: (storyId: string, diceMode: DiceMode, attributesEnabled: boolean) =>
    invoke<void>("save_story_mechanics_settings", { storyId, diceMode, attributesEnabled }),
};
