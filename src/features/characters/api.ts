import { invoke } from "@tauri-apps/api/core";
import type { AttributeRegistryEntry, Entity, EntityAttributeValue, EntityKind } from "../../shared/types";

export const charactersApi = {
  list: (storyId: string) => invoke<Entity[]>("list_entities", { storyId, kind: "character" }),
  create: (storyId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("create_entity", { storyId, kind: "character" satisfies EntityKind, name, appearanceAnchor: appearanceAnchor ?? null }),
  update: (storyId: string, entityId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("update_entity", { storyId, entityId, name, appearanceAnchor: appearanceAnchor ?? null }),
  delete: (storyId: string, entityId: string) => invoke<void>("delete_entity", { storyId, entityId }),
  listAttributes: (storyId: string, entityId: string) => invoke<EntityAttributeValue[]>("list_entity_attributes", { storyId, entityId }),
  listRegistry: () => invoke<AttributeRegistryEntry[]>("list_attribute_registry"),
  setAttribute: (storyId: string, entityId: string, attributeId: string, value: number) => invoke<EntityAttributeValue>("set_entity_attribute", { storyId, entityId, attributeId, value }),
  removeAttribute: (storyId: string, entityId: string, attributeId: string) => invoke<void>("remove_entity_attribute", { storyId, entityId, attributeId }),
};
