import { invoke } from "@tauri-apps/api/core";
import type { MechanicsSettings, Story } from "../../shared/types";

export const storiesApi = {
  list: () => invoke<Story[]>("list_stories"),
  create: (title?: string, settings?: MechanicsSettings | null) =>
    invoke<Story>("create_story", { title: title ?? null, settings: settings ?? null }),
  rename: (storyId: string, title: string) => invoke<void>("rename_story", { storyId, title }),
  delete: (storyId: string) => invoke<void>("delete_story", { storyId }),
};
