import { create, type StoreApi, type UseBoundStore } from "zustand";

export interface SettingsApi<T, Args extends unknown[]> {
  get: () => Promise<T>;
  save: (...args: Args) => Promise<void>;
}

export interface SettingsState<T, Args extends unknown[]> {
  settings: T | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  load: () => Promise<void>;
  save: (...args: Args) => Promise<void>;
}

export type SettingsStore<T, Args extends unknown[]> = UseBoundStore<StoreApi<SettingsState<T, Args>>>;

export function createSettingsStore<T, Args extends unknown[]>(
  api: SettingsApi<T, Args>,
  label: string,
): SettingsStore<T, Args> {
  let requestVersion = 0;
  return create<SettingsState<T, Args>>((set) => ({
    settings: null,
    loading: false,
    saving: false,
    error: null,
    load: async () => {
      const version = ++requestVersion;
      set({ loading: true });
      try {
        const settings = await api.get();
        if (version === requestVersion) set({ settings, loading: false });
      } catch (error) {
        if (version === requestVersion) {
          console.error(`failed to load ${label} settings`, error);
          set({ loading: false });
        }
      }
    },
    save: async (...args) => {
      ++requestVersion;
      set({ saving: true, loading: false, error: null });
      try {
        await api.save(...args);
        const version = ++requestVersion;
        set({ loading: false });
        const settings = await api.get();
        if (version === requestVersion) set({ settings, saving: false });
        else set({ saving: false });
      } catch (error) {
        set({ saving: false, error: String(error) });
        throw error;
      }
    },
  }));
}
