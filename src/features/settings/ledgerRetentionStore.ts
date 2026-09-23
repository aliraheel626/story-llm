import type { LedgerRetentionSettings } from "../../shared/types";
import { ledgerRetentionApi } from "./api";
import { createSettingsStore } from "./settingsStore";

export const useLedgerRetentionStore = createSettingsStore<
  LedgerRetentionSettings,
  Parameters<typeof ledgerRetentionApi.save>
>(ledgerRetentionApi, "ledger retention");
