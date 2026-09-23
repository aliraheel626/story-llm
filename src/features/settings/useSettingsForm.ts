import { useEffect, useRef, useState } from "react";
import type { SettingsStore } from "./settingsStore";

export function useSettingsForm<T, Args extends unknown[]>(
  store: SettingsStore<T, Args>,
  applySettings: (settings: T) => void,
  skipCachedLoad = false,
) {
  const settings = store((state) => state.settings);
  const loading = store((state) => state.loading);
  const saving = store((state) => state.saving);
  const load = store((state) => state.load);
  const saveSettings = store((state) => state.save);
  const applySettingsRef = useRef(applySettings);
  const noticeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [savedNotice, setSavedNotice] = useState(false);

  applySettingsRef.current = applySettings;

  useEffect(() => {
    if (!skipCachedLoad || !store.getState().settings) load();
  }, [load, skipCachedLoad, store]);

  useEffect(() => {
    if (settings) applySettingsRef.current(settings);
  }, [settings]);

  useEffect(
    () => () => {
      if (noticeTimer.current) clearTimeout(noticeTimer.current);
    },
    [],
  );

  const save = async (...args: Args) => {
    try {
      await saveSettings(...args);
      setSavedNotice(true);
      if (noticeTimer.current) clearTimeout(noticeTimer.current);
      noticeTimer.current = setTimeout(() => setSavedNotice(false), 2000);
      return true;
    } catch (error) {
      console.error(error);
      return false;
    }
  };

  return { settings, loading, saving, savedNotice, save };
}
