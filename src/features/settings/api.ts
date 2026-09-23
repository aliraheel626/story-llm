import { invoke } from "@tauri-apps/api/core";
import type {
  ContextInjectionSettings,
  EntityContextMode,
  ImageModelSettings,
  TextModelSettings,
  LedgerRetentionSettings,
} from "../../shared/types";

export const textModelApi = {
  get: () => invoke<TextModelSettings>("get_text_model_settings"),
  save: (provider: string, model: string, apiKey?: string) =>
    invoke<void>("save_text_model_settings", { provider, model, apiKey: apiKey ?? null }),
};

export const imageModelApi = {
  get: () => invoke<ImageModelSettings>("get_image_model_settings"),
  save: (model: string, enabled: boolean, style: string) =>
    invoke<void>("save_image_model_settings", { model, enabled, style }),
};

export const contextInjectionApi = {
  get: () => invoke<ContextInjectionSettings>("get_context_injection_settings"),
  save: (entityContextMode: EntityContextMode, diceRollsInContext: boolean) =>
    invoke<void>("save_context_injection_settings", { entityContextMode, diceRollsInContext }),
};

export const ledgerRetentionApi = {
  get: () => invoke<LedgerRetentionSettings>("get_ledger_retention_settings"),
  save: (toolCallPersistence: boolean) =>
    invoke<void>("save_ledger_retention_settings", { toolCallPersistence }),
};
