import { invoke } from "@tauri-apps/api/core";
import type { AttributeRegistryEntry, Entity, EntityAttributeValue, EntityKind } from "../../shared/types";

export const charactersApi = {
  list: (storyId: string, branchId: string) => invoke<Entity[]>("list_entities", { storyId, branchId, kind: "character" }),
  create: (storyId: string, branchId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("create_entity", { storyId, branchId, kind: "character" satisfies EntityKind, name, appearanceAnchor: appearanceAnchor ?? null }),
  update: (branchId: string, entityId: string, name: string, appearanceAnchor?: string) =>
    invoke<Entity>("update_entity", { branchId, entityId, name, appearanceAnchor: appearanceAnchor ?? null }),
  delete: (branchId: string, entityId: string) => invoke<void>("delete_entity", { branchId, entityId }),
  listAttributes: (branchId: string, entityId: string) => invoke<EntityAttributeValue[]>("list_entity_attributes", { branchId, entityId }),
  listRegistry: () => invoke<AttributeRegistryEntry[]>("list_attribute_registry"),
  setAttribute: (branchId: string, entityId: string, attributeId: string, value: number) => invoke<EntityAttributeValue>("set_entity_attribute", { branchId, entityId, attributeId, value }),
  removeAttribute: (branchId: string, entityId: string, attributeId: string) => invoke<void>("remove_entity_attribute", { branchId, entityId, attributeId }),
};
