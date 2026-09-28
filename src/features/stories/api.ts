import { invoke } from "@tauri-apps/api/core";
import type { Story } from "../../shared/types";

export const storiesApi = {
  list: () => invoke<Story[]>("list_stories"),
  openNew: () => invoke<Story>("new_story"),
  rename: (storyId: string, title: string) => invoke<void>("rename_story", { storyId, title }),
  delete: (storyId: string) => invoke<void>("delete_story", { storyId }),
};
