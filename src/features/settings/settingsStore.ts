import { create, type StoreApi, type UseBoundStore } from "zustand";

export interface SettingsApi<T, Args extends unknown[]> {
  get: () => Promise<T>;
  save: (...args: Args) => Promise<void>;
}

export interface SettingsState<T, Args extends unknown[]> {
  settings: T | null;
  loading: boolean;
  saving: boolean;
  load: () => Promise<void>;
  save: (...args: Args) => Promise<void>;
}

export type SettingsStore<T, Args extends unknown[]> = UseBoundStore<StoreApi<SettingsState<T, Args>>>;

export function createSettingsStore<T, Args extends unknown[]>(
  api: SettingsApi<T, Args>,
  label: string,
): SettingsStore<T, Args> {
  return create<SettingsState<T, Args>>((set) => ({
    settings: null,
    loading: false,
    saving: false,
    load: async () => {
      set({ loading: true });
      try {
        set({ settings: await api.get(), loading: false });
      } catch (error) {
        console.error(`failed to load ${label} settings`, error);
        set({ loading: false });
      }
    },
    save: async (...args) => {
      set({ saving: true });
      try {
        await api.save(...args);
        set({ settings: await api.get(), saving: false });
      } catch (error) {
        set({ saving: false });
        throw error;
      }
    },
  }));
}
