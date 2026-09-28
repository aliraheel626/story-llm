import { AccordionPanel } from "../../shared/ui/AccordionPanel";
import { StoriesPanel } from "../stories/StoriesPanel";
import { CharactersPanel } from "../characters/CharactersPanel";
import { NarratorToolsPanel } from "../narratorTools/NarratorToolsPanel";
import { ContextPanel } from "../context/ContextPanel";
import { InjectionPanel } from "../context/InjectionPanel";
import { TextModelPanel } from "../settings/TextModelPanel";
import { ImageModelPanel } from "../settings/ImageModelPanel";
import { PlaceholderPanel } from "./PlaceholderPanel";
import { useAppShellStore } from "../../app/store";
import { useStoryStore } from "../story/store";

const PANEL_LABELS: Record<string, string> = {
  stories: "Stories",
  characters: "Characters",
  narratorTools: "Narrator Tools",
  context: "Context",
  injection: "Injection",
  textModel: "Text Model",
  imageModel: "Image Model",
  features: "Features",
};

export function Sidebar() {
  const openPanels = useAppShellStore((s) => s.openPanels);
  const togglePanel = useAppShellStore((s) => s.togglePanel);
  const sidebarOpen = useAppShellStore((s) => s.sidebarOpen);
  const toggleSidebar = useAppShellStore((s) => s.toggleSidebar);
  const newStory = useStoryStore((s) => s.newStory);
  const creatingStory = useStoryStore((s) => s.creatingStory);

  const toggleLabel = `${sidebarOpen ? "Close" : "Open"} sidebar (Ctrl+B)`;

  return (
    <aside
      aria-label="Sidebar"
      className={`flex h-full shrink-0 flex-col overflow-hidden border-r border-border bg-surface transition-[width] duration-150 motion-reduce:transition-none ${sidebarOpen ? "w-64" : "w-12"}`}
    >
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-2">
        <button
          type="button"
          aria-label={toggleLabel}
          title={toggleLabel}
          aria-expanded={sidebarOpen}
          aria-controls={sidebarOpen ? "sidebar-panels" : undefined}
          onClick={toggleSidebar}
          className="shrink-0 rounded p-2 text-muted hover:bg-bg hover:text-text"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" aria-hidden="true">
            <rect x="3" y="4" width="18" height="16" rx="2" />
            <path d="M9 4v16" />
          </svg>
        </button>
        {sidebarOpen && <span className="truncate font-prose text-sm tracking-wide text-text">story-llm</span>}
      </div>

      {!sidebarOpen && (
        <button
          type="button"
          aria-label="New story"
          title="New story"
          disabled={creatingStory}
          onClick={() => newStory().catch((err) => console.error("failed to open a new story", err))}
          className="mx-auto mt-2 shrink-0 rounded p-2 text-muted hover:bg-bg hover:text-text disabled:opacity-40"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M12 20h9" />
            <path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" />
          </svg>
        </button>
      )}

      {sidebarOpen && (
        <div id="sidebar-panels" className="w-64 flex-1 overflow-y-auto">
          <AccordionPanel title={PANEL_LABELS.stories} open={openPanels.stories} onToggle={() => togglePanel("stories")}>
            <StoriesPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.characters} open={openPanels.characters} onToggle={() => togglePanel("characters")}>
            <CharactersPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.narratorTools} open={openPanels.narratorTools} onToggle={() => togglePanel("narratorTools")}>
            <NarratorToolsPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.context} open={openPanels.context} onToggle={() => togglePanel("context")}>
            <ContextPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.injection} open={openPanels.injection} onToggle={() => togglePanel("injection")}>
            <InjectionPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.textModel} open={openPanels.textModel} onToggle={() => togglePanel("textModel")}>
            <TextModelPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.imageModel} open={openPanels.imageModel} onToggle={() => togglePanel("imageModel")}>
            <ImageModelPanel />
          </AccordionPanel>

          <AccordionPanel title={PANEL_LABELS.features} open={openPanels.features} onToggle={() => togglePanel("features")}>
            <PlaceholderPanel label="Features" />
          </AccordionPanel>
        </div>
      )}
    </aside>
  );
}
