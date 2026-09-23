import { create } from "zustand";
import { DEFAULT_NARRATOR_TOOLS } from "../../shared/types";
import type {
  ActionMode,
  Entity,
  NarratorToolSettings,
  NarrationDonePayload,
  NarrationToolActivityPayload,
  ReasoningEffort,
  Story,
  StoryImage,
  LedgerEntry,
} from "../../shared/types";
import { charactersApi } from "../characters/api";
import { narratorToolsApi } from "../narratorTools/api";
import { storiesApi } from "../stories/api";
import { ledgerApi } from "../ledger/api";
import { writingStyleApi } from "../writingStyle/api";

type ToolActivity = {
  callId: string;
  label: string;
  phase: "started" | "finished";
  ok: boolean | null;
};

interface StreamingState {
  streamId: string;
  storyId: string;
  text: string;
  thoughts: string;
  mode: "append" | "replace";
  targetEntryId?: string;
  toolActivity?: ToolActivity | null;
  toolLog: ToolActivity[];
}

interface TurnActivity {
  entryId: string;
  thoughts: string;
  tools: ToolActivity[];
}

export interface StoryBundle {
  id: string;
  entries: LedgerEntry[];
  hidden: LedgerEntry[];
  ledgerLoading: boolean;
  requestPending: boolean;
  streaming: StreamingState | null;
  turnError: string | null;
  turnActivity: TurnActivity | null;
  imagesByEntry: Record<string, StoryImage[]>;
  imagePendingFor: string[];
  imageError: string | null;
  characters: Entity[];
  charactersLoading: boolean;
  narratorTools: NarratorToolSettings | null;
  narratorToolsLoading: boolean;
  narratorToolsError: string | null;
  reasoningEffort: ReasoningEffort | null;
  reasoningEffortLoaded: boolean;
  reasoningEffortError: string | null;
  authorNote: string | null;
  authorNoteLoading: boolean;
  authorNoteSaving: boolean;
}

interface StoryStoreState {
  stories: Story[];
  storiesLoading: boolean;
  creatingStory: boolean;
  activeStoryId: string | null;
  draft: boolean;
  draftNarratorTools: NarratorToolSettings;
  draftReasoningEffort: ReasoningEffort | null;
  bundles: Record<string, StoryBundle>;

  loadStories: () => Promise<void>;
  startDraft: () => void;
  createStory: () => Promise<Story>;
  renameStory: (storyId: string, title: string) => Promise<void>;
  deleteStory: (storyId: string) => Promise<void>;
  applyStoryTitle: (storyId: string, title: string) => void;
  setActiveStory: (storyId: string) => void;

  loadLedger: (storyId: string) => Promise<void>;
  submitTurn: (storyId: string, mode: ActionMode, content: string) => Promise<void>;
  retryNarration: (storyId: string, entryId: string) => Promise<void>;
  eraseLastExchange: (storyId: string) => Promise<void>;
  editEntry: (storyId: string, entryId: string, content: string) => Promise<void>;
  loadImagesForStory: (storyId: string) => Promise<void>;
  _appendDelta: (streamId: string, text: string) => void;
  _appendThoughts: (streamId: string, text: string) => void;
  _toolActivity: (payload: NarrationToolActivityPayload) => void;
  _finalize: (payload: NarrationDonePayload) => void;
  _fail: (streamId: string, message: string) => void;
  _imagePending: (entryId: string) => void;
  _imageGenerated: (image: StoryImage) => void;
  _imageFailed: (entryId: string) => void;

  loadCharacters: (storyId: string) => Promise<void>;
  createCharacter: (storyId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  updateCharacter: (storyId: string, entityId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  deleteCharacter: (storyId: string, entityId: string) => Promise<void>;

  loadNarratorTools: (storyId: string) => Promise<void>;
  saveNarratorTools: (storyId: string | null, patch: Partial<NarratorToolSettings>) => Promise<void>;
  loadReasoningEffort: (storyId: string) => Promise<void>;
  saveReasoningEffort: (storyId: string | null, value: ReasoningEffort | null) => Promise<void>;

  loadAuthorNote: (storyId: string) => Promise<void>;
  saveAuthorNote: (storyId: string, note: string) => Promise<void>;
}

const newBundle = (id: string): StoryBundle => ({
  id,
  entries: [],
  hidden: [],
  ledgerLoading: false,
  requestPending: false,
  streaming: null,
  turnError: null,
  turnActivity: null,
  imagesByEntry: {},
  imagePendingFor: [],
  imageError: null,
  characters: [],
  charactersLoading: false,
  narratorTools: null,
  narratorToolsLoading: false,
  narratorToolsError: null,
  reasoningEffort: null,
  reasoningEffortLoaded: false,
  reasoningEffortError: null,
  authorNote: null,
  authorNoteLoading: false,
  authorNoteSaving: false,
});

const patchBundle = (
  bundles: Record<string, StoryBundle>,
  storyId: string,
  patch: Partial<StoryBundle> | ((bundle: StoryBundle) => Partial<StoryBundle>),
) => {
  const bundle = bundles[storyId] ?? newBundle(storyId);
  return { ...bundles, [storyId]: { ...bundle, ...(typeof patch === "function" ? patch(bundle) : patch) } };
};

const replaceEntry = (entries: LedgerEntry[], id: string, next: LedgerEntry) =>
  entries.map((entry) => (entry.id === id ? next : entry));

const removeOne = (items: string[], value: string) => {
  const index = items.indexOf(value);
  return index < 0 ? items : items.slice(0, index).concat(items.slice(index + 1));
};

const newStream = (
  streamId: string,
  storyId: string,
  mode: "append" | "replace",
  targetEntryId?: string,
): StreamingState => ({ streamId, storyId, text: "", thoughts: "", mode, targetEntryId, toolLog: [] });

const findStream = (bundles: Record<string, StoryBundle>, streamId: string): [string, StreamingState] | undefined => {
  for (const [storyId, bundle] of Object.entries(bundles)) {
    if (bundle.streaming?.streamId === streamId) return [storyId, bundle.streaming];
  }
  return undefined;
};

const findStoryForEntry = (bundles: Record<string, StoryBundle>, entryId: string): string | undefined => {
  for (const [storyId, bundle] of Object.entries(bundles)) {
    if (
      bundle.entries.some((entry) => entry.id === entryId) ||
      bundle.hidden.some((entry) => entry.id === entryId) ||
      bundle.imagePendingFor.includes(entryId) ||
      Object.prototype.hasOwnProperty.call(bundle.imagesByEntry, entryId)
    ) {
      return storyId;
    }
  }
  return undefined;
};

const closeTool = (log: ToolActivity[], callId: string, ok: boolean | null): ToolActivity[] => {
  const open = log.findIndex((tool) => tool.callId === callId && tool.phase === "started");
  return open < 0
    ? log
    : log.map((tool, index) => (index === open ? { ...tool, phase: "finished" as const, ok } : tool));
};

const ledgerGenerations = new Map<string, number>();
const imageGenerations = new Map<string, number>();
const imageEventGenerations = new Map<string, number>();
const advanceGeneration = (generations: Map<string, number>, key: string) => {
  const generation = (generations.get(key) ?? 0) + 1;
  generations.set(key, generation);
  return generation;
};
const isCurrentGeneration = (generations: Map<string, number>, key: string, generation: number) =>
  generations.get(key) === generation;
const settingsQueues = new Map<string, Promise<void>>();
const settingsLoads = new Map<string, Promise<void>>();
const reasoningLoads = new Map<string, Promise<void>>();
const reasoningQueues = new Map<string, Promise<void>>();
const enqueueSettings = (queues: Map<string, Promise<void>>, storyId: string, operation: () => Promise<void>): Promise<void> => {
  const previous = queues.get(storyId) ?? Promise.resolve();
  const current = previous.catch(() => undefined).then(operation);
  queues.set(storyId, current);
  return current.finally(() => {
    if (queues.get(storyId) === current) queues.delete(storyId);
  });
};

export const useStoryStore = create<StoryStoreState>((set, get) => ({
  stories: [],
  storiesLoading: false,
  creatingStory: false,
  activeStoryId: null,
  draft: false,
  draftNarratorTools: { ...DEFAULT_NARRATOR_TOOLS },
  draftReasoningEffort: null,
  bundles: {},

  loadStories: async () => {
    set({ storiesLoading: true });
    try {
      set({ stories: await storiesApi.list(), storiesLoading: false });
    } catch (error) {
      console.error("failed to load stories", error);
      set({ storiesLoading: false });
    }
  },
  startDraft: () => set({ activeStoryId: null, draft: true, draftNarratorTools: { ...DEFAULT_NARRATOR_TOOLS }, draftReasoningEffort: null }),
  createStory: async () => {
    const { draftNarratorTools, draftReasoningEffort } = get();
    set({ creatingStory: true });
    try {
      const story = await storiesApi.create(undefined, {
        narrator_tools: draftNarratorTools,
        ...(draftReasoningEffort ? { reasoning_effort: draftReasoningEffort } : {}),
      });
      set((state) => ({
        stories: [story, ...state.stories],
        activeStoryId: story.id,
        draft: false,
        draftNarratorTools: { ...DEFAULT_NARRATOR_TOOLS },
        draftReasoningEffort: null,
        bundles: patchBundle(state.bundles, story.id, { narratorTools: draftNarratorTools, reasoningEffort: draftReasoningEffort, reasoningEffortLoaded: true }),
      }));
      return story;
    } finally {
      set({ creatingStory: false });
    }
  },
  renameStory: async (storyId, title) => {
    await storiesApi.rename(storyId, title);
    set((state) => ({ stories: state.stories.map((story) => (story.id === storyId ? { ...story, title } : story)) }));
  },
  deleteStory: async (storyId) => {
    await storiesApi.delete(storyId);
    set((state) => {
      const bundles = { ...state.bundles };
      delete bundles[storyId];
      return {
        bundles,
        stories: state.stories.filter((story) => story.id !== storyId),
        activeStoryId: state.activeStoryId === storyId ? null : state.activeStoryId,
      };
    });
  },
  applyStoryTitle: (storyId, title) =>
    set((state) => ({ stories: state.stories.map((story) => (story.id === storyId ? { ...story, title } : story)) })),
  setActiveStory: (storyId) => {
    if (get().activeStoryId !== storyId || get().draft) {
      set({ activeStoryId: storyId, draft: false, draftNarratorTools: { ...DEFAULT_NARRATOR_TOOLS }, draftReasoningEffort: null });
    }
  },

  loadLedger: async (storyId) => {
    const generation = advanceGeneration(ledgerGenerations, storyId);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { ledgerLoading: true }) }));
    try {
      const snapshot = await ledgerApi.list(storyId);
      if (!isCurrentGeneration(ledgerGenerations, storyId, generation)) return;
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, { entries: snapshot.visible, hidden: snapshot.hidden }),
      }));
      if (isCurrentGeneration(ledgerGenerations, storyId, generation)) {
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { ledgerLoading: false }) }));
      }
    } catch (error) {
      console.error("failed to load ledger", error);
      if (isCurrentGeneration(ledgerGenerations, storyId, generation)) {
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { ledgerLoading: false }) }));
      }
    }
  },
  submitTurn: async (storyId, mode, content) => {
    if (get().bundles[storyId]?.requestPending || get().bundles[storyId]?.streaming) throw new Error("A narration request is already in progress");
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { requestPending: true }) }));
    advanceGeneration(ledgerGenerations, storyId);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null, ledgerLoading: false }) }));
    try {
      await Promise.all([settingsQueues.get(storyId), reasoningQueues.get(storyId)]);
      const result = await ledgerApi.submitTurn(storyId, mode, content);
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          entries: bundle.entries.some((entry) => entry.id === result.entry.id)
            ? replaceEntry(bundle.entries, result.entry.id, result.entry)
            : [...bundle.entries, result.entry],
          streaming: newStream(result.stream_id, storyId, "append"),
          ledgerLoading: false,
        })),
      }));
    } catch (error) {
      await get().loadLedger(storyId);
      throw error;
    } finally {
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { requestPending: false }) }));
    }
  },
  retryNarration: async (storyId, entryId) => {
    if (get().bundles[storyId]?.requestPending || get().bundles[storyId]?.streaming) throw new Error("A narration request is already in progress");
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null, requestPending: true }) }));
    const appends = get().bundles[storyId]?.entries.find((entry) => entry.id === entryId)?.kind === "player_message";
    try {
      await Promise.all([settingsQueues.get(storyId), reasoningQueues.get(storyId)]);
      const result = await ledgerApi.retry(storyId, entryId);
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, {
          streaming: newStream(result.stream_id, storyId, appends ? "append" : "replace", appends ? undefined : entryId),
        }),
      }));
    } catch (error) {
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: String(error) }) }));
      throw error;
    } finally {
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { requestPending: false }) }));
    }
  },
  eraseLastExchange: async (storyId) => {
    const ids = await ledgerApi.eraseLastExchange(storyId);
    if (!ids.length) return;
    advanceGeneration(ledgerGenerations, storyId);
    advanceGeneration(imageGenerations, storyId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => {
        const imagesByEntry = { ...bundle.imagesByEntry };
        ids.forEach((entryId) => {
          delete imagesByEntry[entryId];
        });
        return {
          entries: bundle.entries.filter((entry) => !ids.includes(entry.id)),
          hidden: bundle.hidden.filter((entry) => !ids.includes(entry.id) && !ids.includes(entry.target_entry_id ?? "")),
          imagesByEntry,
          imagePendingFor: bundle.imagePendingFor.filter((entryId) => !ids.includes(entryId)),
          ledgerLoading: false,
        };
      }),
    }));
    await get().loadCharacters(storyId);
    await get().loadImagesForStory(storyId);
  },
  editEntry: async (storyId, entryId, content) => {
    const entry = await ledgerApi.edit(entryId, content);
    advanceGeneration(ledgerGenerations, storyId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        entries: replaceEntry(bundle.entries, entryId, entry),
        imagesByEntry: { ...bundle.imagesByEntry, [entryId]: [] },
        ledgerLoading: false,
      })),
    }));
  },
  loadImagesForStory: async (storyId) => {
    const generation = advanceGeneration(imageGenerations, storyId);
    const eventGeneration = imageEventGenerations.get(storyId);
    try {
      const images = await ledgerApi.listImages(storyId);
      if (!isCurrentGeneration(imageGenerations, storyId, generation)) return;
      const grouped: Record<string, StoryImage[]> = {};
      images.forEach((image) => (grouped[image.entry_id] ??= []).push(image));
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, (bundle) => {
        if (imageEventGenerations.get(storyId) !== eventGeneration) {
          for (const [entryId, current] of Object.entries(bundle.imagesByEntry)) {
            if (!bundle.entries.some((entry) => entry.id === entryId)) continue;
            const images = grouped[entryId] ?? [];
            grouped[entryId] = [...images, ...current.filter((image) => !images.some((candidate) => candidate.id === image.id))];
          }
        }
        return { imagesByEntry: grouped };
      }) }));
    } catch (error) {
      console.error("failed to load images", error);
    }
  },
  _appendDelta: (streamId, text) => {
    const found = findStream(get().bundles, streamId);
    if (!found) return;
    const [storyId] = found;
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) =>
        bundle.streaming?.streamId === streamId ? { streaming: { ...bundle.streaming, text: bundle.streaming.text + text } } : {}),
    }));
  },
  _appendThoughts: (streamId, text) => {
    const found = findStream(get().bundles, streamId);
    if (!found) return;
    const [storyId] = found;
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) =>
        bundle.streaming?.streamId === streamId
          ? { streaming: { ...bundle.streaming, thoughts: bundle.streaming.thoughts + text } }
          : {}),
    }));
  },
  _toolActivity: (payload) => {
    const found = findStream(get().bundles, payload.stream_id);
    if (!found) return;
    const [storyId, current] = found;
    let toolLog: ToolActivity[];
    if (payload.phase === "started") {
      toolLog = [...current.toolLog, { callId: payload.call_id, label: payload.label, phase: "started", ok: null }];
    } else {
      const closed = closeTool(current.toolLog, payload.call_id, payload.ok);
      toolLog = closed === current.toolLog
        ? [...current.toolLog, { callId: payload.call_id, label: payload.label, phase: "finished", ok: payload.ok }]
        : closed;
    }
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) =>
        bundle.streaming?.streamId === payload.stream_id
          ? {
              streaming: {
                ...bundle.streaming,
                toolLog,
                toolActivity: {
                  callId: payload.call_id,
                  label: payload.label,
                  phase: payload.phase,
                  ok: payload.ok,
                },
              },
            }
          : {}),
    }));
  },
  _finalize: (payload) => {
    const found = findStream(get().bundles, payload.stream_id);
    if (!found) return;
    const [storyId, current] = found;
    advanceGeneration(ledgerGenerations, storyId);
    advanceGeneration(imageGenerations, storyId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => {
        const oldId = current.mode === "replace" ? current.targetEntryId : undefined;
        const imagesByEntry = { ...bundle.imagesByEntry };
        if (oldId) {
          delete imagesByEntry[oldId];
        }
        return {
          entries: oldId
            ? replaceEntry(bundle.entries, oldId, payload.entry)
            : bundle.entries.some((entry) => entry.id === payload.entry.id)
              ? replaceEntry(bundle.entries, payload.entry.id, payload.entry)
              : [...bundle.entries, payload.entry],
          hidden: oldId ? bundle.hidden.filter((entry) => entry.id !== oldId && entry.target_entry_id !== oldId) : bundle.hidden,
          imagesByEntry,
          imagePendingFor: oldId ? removeOne(bundle.imagePendingFor, oldId) : bundle.imagePendingFor,
          streaming: null,
          turnActivity: payload.entry.kind === "narration"
            ? { entryId: payload.entry.id, thoughts: current.thoughts, tools: current.toolLog }
            : null,
          ledgerLoading: false,
        };
      }),
    }));
    get().loadLedger(storyId);
    get().loadImagesForStory(storyId);
    get().loadCharacters(storyId);
  },
  _fail: (streamId, message) => {
    const found = findStream(get().bundles, streamId);
    if (!found) return;
    const [storyId, current] = found;
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, {
        streaming: null,
        turnActivity: current.mode === "replace" && current.targetEntryId
          ? { entryId: current.targetEntryId, thoughts: "", tools: [] }
          : null,
        turnError: message,
      }),
    }));
  },
  _imagePending: (entryId) => {
    const storyId = findStoryForEntry(get().bundles, entryId);
    if (!storyId) return;
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        imagePendingFor: bundle.imagePendingFor.includes(entryId) ? bundle.imagePendingFor : [...bundle.imagePendingFor, entryId],
      })),
    }));
  },
  _imageGenerated: (image) => {
    const storyId = findStoryForEntry(get().bundles, image.entry_id);
    if (!storyId) return;
    advanceGeneration(imageEventGenerations, storyId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => {
        const current = bundle.imagesByEntry[image.entry_id] ?? [];
        return {
          imagesByEntry: {
            ...bundle.imagesByEntry,
            [image.entry_id]: current.some((candidate) => candidate.id === image.id) ? current : [...current, image],
          },
          imagePendingFor: removeOne(bundle.imagePendingFor, image.entry_id),
        };
      }),
    }));
  },
  _imageFailed: (entryId) => {
    const storyId = findStoryForEntry(get().bundles, entryId);
    if (!storyId) return;
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        imagePendingFor: removeOne(bundle.imagePendingFor, entryId),
      })),
    }));
  },

  loadCharacters: async (storyId) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { charactersLoading: true }) }));
    try {
      const characters = await charactersApi.list(storyId);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { characters, charactersLoading: false }) }));
    } catch (error) {
      console.error("failed to load characters", error);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { charactersLoading: false }) }));
    }
  },
  createCharacter: async (storyId, name, appearanceAnchor) => {
    const character = await charactersApi.create(storyId, name, appearanceAnchor);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({ characters: [...bundle.characters, character] })),
    }));
  },
  updateCharacter: async (storyId, entityId, name, appearanceAnchor) => {
    const character = await charactersApi.update(storyId, entityId, name, appearanceAnchor);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        characters: bundle.characters.map((entity) => (entity.id === entityId ? character : entity)),
      })),
    }));
  },
  deleteCharacter: async (storyId, entityId) => {
    await charactersApi.delete(storyId, entityId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        characters: bundle.characters.filter((entity) => entity.id !== entityId),
      })),
    }));
  },

  loadNarratorTools: async (storyId) => {
    const activeLoad = settingsLoads.get(storyId);
    if (activeLoad) return activeLoad;
    if (get().bundles[storyId]?.narratorTools) return;
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { narratorToolsLoading: true, narratorToolsError: null }) }));
    const load = enqueueSettings(settingsQueues, storyId, async () => {
      const narratorTools = await narratorToolsApi.get(storyId);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { narratorTools }) }));
    })
      .catch((error) => set((state) => ({ bundles: patchBundle(state.bundles, storyId, { narratorToolsError: String(error) }) })))
      .finally(() => {
        if (settingsLoads.get(storyId) === load) settingsLoads.delete(storyId);
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { narratorToolsLoading: false }) }));
      });
    settingsLoads.set(storyId, load);
    return load;
  },
  saveNarratorTools: async (storyId, patch) => {
    if (!storyId) {
      set((state) => ({ draftNarratorTools: { ...state.draftNarratorTools, ...patch } }));
      return;
    }
    const loaded = get().bundles[storyId]?.narratorTools;
    if (!loaded) throw new Error("Narrator tools are not loaded yet");
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, (bundle) => ({
      narratorTools: { ...bundle.narratorTools!, ...patch }, narratorToolsError: null,
    })) }));
    try {
      await enqueueSettings(settingsQueues, storyId, async () => {
        const current = await narratorToolsApi.get(storyId);
        await narratorToolsApi.save(storyId, { ...current, ...patch });
      });
    } catch (error) {
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { narratorToolsError: String(error) }) }));
      await enqueueSettings(settingsQueues, storyId, async () => {
        const narratorTools = await narratorToolsApi.get(storyId);
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { narratorTools }) }));
      });
      throw error;
    }
  },
  loadReasoningEffort: async (storyId) => {
    const activeLoad = reasoningLoads.get(storyId);
    if (activeLoad) return activeLoad;
    if (get().bundles[storyId]?.reasoningEffortLoaded) return;
    const load = enqueueSettings(reasoningQueues, storyId, async () => {
      const reasoningEffort = await narratorToolsApi.getReasoningEffort(storyId);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { reasoningEffort, reasoningEffortLoaded: true, reasoningEffortError: null }) }));
    })
      .catch((error) => set((state) => ({ bundles: patchBundle(state.bundles, storyId, { reasoningEffortError: String(error) }) })))
      .finally(() => { if (reasoningLoads.get(storyId) === load) reasoningLoads.delete(storyId); });
    reasoningLoads.set(storyId, load);
    return load;
  },
  saveReasoningEffort: async (storyId, value) => {
    if (!storyId) {
      set({ draftReasoningEffort: value });
      return;
    }
    if (!get().bundles[storyId]?.reasoningEffortLoaded) throw new Error("Reasoning effort is not loaded yet");
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { reasoningEffort: value, reasoningEffortError: null }) }));
    try {
      await enqueueSettings(reasoningQueues, storyId, () => narratorToolsApi.saveReasoningEffort(storyId, value));
    } catch (error) {
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { reasoningEffortError: String(error) }) }));
      await enqueueSettings(reasoningQueues, storyId, async () => {
        const reasoningEffort = await narratorToolsApi.getReasoningEffort(storyId);
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { reasoningEffort }) }));
      });
      throw error;
    }
  },

  loadAuthorNote: async (storyId) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { authorNoteLoading: true }) }));
    try {
      const authorNote = await writingStyleApi.getAuthorNote(storyId);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { authorNote, authorNoteLoading: false }) }));
    } catch (error) {
      console.error("failed to load author's note", error);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { authorNoteLoading: false }) }));
    }
  },
  saveAuthorNote: async (storyId, note) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { authorNoteSaving: true }) }));
    try {
      await writingStyleApi.saveAuthorNote(storyId, note);
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, { authorNote: note.trim(), authorNoteSaving: false }),
      }));
    } catch (error) {
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { authorNoteSaving: false }) }));
      throw error;
    }
  },
}));
