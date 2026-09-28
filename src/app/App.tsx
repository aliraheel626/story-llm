import { useEffect } from "react";
import { Sidebar } from "../features/layout/Sidebar";
import { StoryView } from "../features/transcript/StoryView";
import { useNarrationEvents } from "./useNarrationEvents";
import { useAppShellStore } from "./store";

function App() {
  useNarrationEvents();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key.toLowerCase() !== "b" || !(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return;
      const target = event.target;
      if (target instanceof Element && target.closest('input, textarea, [contenteditable]:not([contenteditable="false"])')) return;
      event.preventDefault();
      useAppShellStore.getState().toggleSidebar();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="flex h-screen w-screen bg-bg text-text">
      <Sidebar />
      <main className="flex flex-1 flex-col overflow-hidden">
        <StoryView />
      </main>
    </div>
  );
}

export default App;
