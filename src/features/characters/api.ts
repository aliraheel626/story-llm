import { invoke } from "@tauri-apps/api/core";
import type { AttributeRegistryEntry, CharacterPatch, Entity, EntityAttributeValue, EntityKind } from "../../shared/types";

export const charactersApi = {
  list: (storyId: string) => invoke<Entity[]>("list_entities", { storyId, kind: null }),
  create: (storyId: string, name: string, fields: CharacterPatch) =>
    invoke<Entity>("create_entity", { storyId, kind: "character" satisfies EntityKind, name, fields }),
  update: (storyId: string, entityId: string, name: string | undefined, fields: CharacterPatch) =>
    invoke<Entity>("update_entity", { storyId, entityId, name: name ?? null, fields }),
  delete: (storyId: string, entityId: string) => invoke<void>("delete_entity", { storyId, entityId }),
  listStoryAttributes: (storyId: string) => invoke<Record<string, EntityAttributeValue[]>>("list_story_attributes", { storyId }),
  listAttributes: (storyId: string, entityId: string) => invoke<EntityAttributeValue[]>("list_entity_attributes", { storyId, entityId }),
  listRegistry: () => invoke<AttributeRegistryEntry[]>("list_attribute_registry"),
  setAttribute: (storyId: string, entityId: string, attributeId: string, value: number) => invoke<EntityAttributeValue>("set_entity_attribute", { storyId, entityId, attributeId, value }),
  removeAttribute: (storyId: string, entityId: string, attributeId: string) => invoke<void>("remove_entity_attribute", { storyId, entityId, attributeId }),
};
