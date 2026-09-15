import { invoke } from "@tauri-apps/api/core";
import type { Entity, EntityKind } from "../../shared/types";

export const charactersApi = {
  list: (storyId: string) => invoke<Entity[]>("list_entities", { storyId, kind: "character" }),
  create: (storyId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("create_entity", { storyId, kind: "character" satisfies EntityKind, name, appearanceAnchor: appearanceAnchor ?? null }),
  update: (entityId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("update_entity", { entityId, name, appearanceAnchor: appearanceAnchor ?? null }),
  delete: (entityId: string) => invoke<void>("delete_entity", { entityId }),
};
