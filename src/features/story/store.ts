import { create } from "zustand";
import type {
  DicerollSettings,
  Entity,
  NarrationDonePayload,
  NarrationToolActivityPayload,
  NarrationVariant,
  RollDetail,
  Story,
  StoryImage,
  SwipeDonePayload,
  TimelineEntry,
} from "../../shared/types";
import { charactersApi } from "../characters/api";
import { dicerollApi } from "../dicerolls/api";
import { storiesApi } from "../stories/api";
import { timelineApi } from "../timeline/api";
import { writingStyleApi } from "../writingStyle/api";

export const DEFAULT_DICEROLL_SETTINGS: DicerollSettings = {
  dice_mode: "classifier",
  attributes_enabled: true,
  reasoning_effort: null,
};

export type DicerollSettingsPatch = Partial<DicerollSettings>;

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
  thoughts: string;
  tools: ToolActivity[];
}

export interface StoryBundle {
  id: string;
  entries: TimelineEntry[];
  hidden: TimelineEntry[];
  timelineLoading: boolean;
  streaming: StreamingState | null;
  turnError: string | null;
  turnActivity: TurnActivity | null;
  variantsByEntry: Record<string, NarrationVariant[]>;
  imagesByEntry: Record<string, StoryImage[]>;
  imagePendingFor: string[];
  imageError: string | null;
  rollsByEntry: Record<string, RollDetail[]>;
  rollDetailByEntry: Record<string, RollDetail[]>;
  characters: Entity[];
  charactersLoading: boolean;
  diceSettings: DicerollSettings | null;
  diceSettingsLoading: boolean;
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
  draftDiceSettings: DicerollSettings | null;
  bundles: Record<string, StoryBundle>;

  loadStories: () => Promise<void>;
  startDraft: () => void;
  createStory: () => Promise<Story>;
  renameStory: (storyId: string, title: string) => Promise<void>;
  deleteStory: (storyId: string) => Promise<void>;
  applyStoryTitle: (storyId: string, title: string) => void;
  setActiveStory: (storyId: string) => void;

  loadTimeline: (storyId: string) => Promise<void>;
  submitStoryText: (storyId: string, content: string) => Promise<void>;
  submitTurn: (storyId: string, mode: "do" | "say", content: string) => Promise<void>;
  submitGuide: (storyId: string, note: string) => Promise<void>;
  continueScene: (storyId: string) => Promise<void>;
  retryNarration: (storyId: string, entryId: string) => Promise<void>;
  generateVariant: (storyId: string, entryId: string) => Promise<void>;
  eraseLastExchange: (storyId: string) => Promise<void>;
  editEntry: (storyId: string, entryId: string, content: string) => Promise<void>;
  selectVariant: (storyId: string, entryId: string, variantEntryId: string) => Promise<void>;
  loadVariantsForEntry: (storyId: string, entryId: string) => Promise<void>;
  loadImagesForStory: (storyId: string) => Promise<void>;
  generateImageForEntry: (storyId: string, entryId: string, hint?: string) => Promise<void>;
  loadRollsForStory: (storyId: string) => Promise<void>;
  loadRollDetail: (storyId: string, entryId: string) => Promise<void>;
  _appendDelta: (streamId: string, text: string) => void;
  _appendThoughts: (streamId: string, text: string) => void;
  _toolActivity: (payload: NarrationToolActivityPayload) => void;
  _finalize: (payload: NarrationDonePayload) => void;
  _swipeDone: (payload: SwipeDonePayload) => void;
  _fail: (streamId: string, message: string) => void;
  _imagePending: (entryId: string) => void;
  _imageGenerated: (image: StoryImage) => void;
  _imageFailed: (entryId: string) => void;

  loadCharacters: (storyId: string) => Promise<void>;
  createCharacter: (storyId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  updateCharacter: (storyId: string, entityId: string, name: string, appearanceAnchor?: string) => Promise<void>;
  deleteCharacter: (storyId: string, entityId: string) => Promise<void>;

  loadDiceSettings: (storyId: string) => Promise<void>;
  saveDiceSettings: (storyId: string | null, patch: DicerollSettingsPatch) => Promise<void>;
  resetDraftDiceSettings: () => void;

  loadAuthorNote: (storyId: string) => Promise<void>;
  saveAuthorNote: (storyId: string, note: string) => Promise<void>;
}

const newBundle = (id: string): StoryBundle => ({
  id,
  entries: [],
  hidden: [],
  timelineLoading: false,
  streaming: null,
  turnError: null,
  turnActivity: null,
  variantsByEntry: {},
  imagesByEntry: {},
  imagePendingFor: [],
  imageError: null,
  rollsByEntry: {},
  rollDetailByEntry: {},
  characters: [],
  charactersLoading: false,
  diceSettings: null,
  diceSettingsLoading: false,
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

const replaceEntry = (entries: TimelineEntry[], id: string, next: TimelineEntry) =>
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

const timelineGenerations = new Map<string, number>();
const variantGenerations = new Map<string, number>();
const advanceGeneration = (generations: Map<string, number>, key: string) => {
  const generation = (generations.get(key) ?? 0) + 1;
  generations.set(key, generation);
  return generation;
};
const isCurrentGeneration = (generations: Map<string, number>, key: string, generation: number) =>
  generations.get(key) === generation;
const variantKey = (storyId: string, entryId: string) => `${storyId}:${entryId}`;

const settingsQueues = new Map<string, Promise<void>>();
const settingsLoads = new Map<string, Promise<void>>();
const enqueueSettings = (storyId: string, operation: () => Promise<void>): Promise<void> => {
  const previous = settingsQueues.get(storyId) ?? Promise.resolve();
  const current = previous.catch(() => undefined).then(operation);
  settingsQueues.set(storyId, current);
  return current.finally(() => {
    if (settingsQueues.get(storyId) === current) settingsQueues.delete(storyId);
  });
};

export const useStoryStore = create<StoryStoreState>((set, get) => ({
  stories: [],
  storiesLoading: false,
  creatingStory: false,
  activeStoryId: null,
  draft: false,
  draftDiceSettings: null,
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
  startDraft: () => set({ activeStoryId: null, draft: true, draftDiceSettings: null }),
  createStory: async () => {
    const draftDiceSettings = get().draftDiceSettings;
    set({ creatingStory: true });
    try {
      const story = await storiesApi.create(undefined, draftDiceSettings);
      set((state) => ({
        stories: [story, ...state.stories],
        activeStoryId: story.id,
        draft: false,
        draftDiceSettings: null,
        bundles: patchBundle(state.bundles, story.id, { diceSettings: draftDiceSettings }),
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
      set({ activeStoryId: storyId, draft: false, draftDiceSettings: null });
    }
  },

  loadTimeline: async (storyId) => {
    const generation = advanceGeneration(timelineGenerations, storyId);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { timelineLoading: true }) }));
    try {
      const snapshot = await timelineApi.list(storyId);
      if (!isCurrentGeneration(timelineGenerations, storyId, generation)) return;
      const revisedEntryIds = [...new Set(snapshot.hidden.flatMap((entry) =>
        (entry.kind === "narration_variant" || entry.kind === "narration_selected") && entry.target_entry_id
          ? [entry.target_entry_id]
          : []))];
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, { entries: snapshot.visible, hidden: snapshot.hidden }),
      }));
      await Promise.all(revisedEntryIds.map((entryId) => get().loadVariantsForEntry(storyId, entryId)));
      if (isCurrentGeneration(timelineGenerations, storyId, generation)) {
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { timelineLoading: false }) }));
      }
    } catch (error) {
      console.error("failed to load timeline", error);
      if (isCurrentGeneration(timelineGenerations, storyId, generation)) {
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { timelineLoading: false }) }));
      }
    }
  },
  submitStoryText: async (storyId, content) => {
    advanceGeneration(timelineGenerations, storyId);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null, timelineLoading: false }) }));
    try {
      const result = await timelineApi.submitStory(storyId, content);
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          entries: [...bundle.entries, result.entry],
          streaming: newStream(result.stream_id, storyId, "append"),
          timelineLoading: false,
        })),
      }));
    } catch (error) {
      await get().loadTimeline(storyId);
      throw error;
    }
  },
  submitTurn: async (storyId, mode, content) => {
    advanceGeneration(timelineGenerations, storyId);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null, timelineLoading: false }) }));
    try {
      const result = await timelineApi.submitTurn(storyId, mode, content);
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          entries: [...bundle.entries, result.entry],
          streaming: newStream(result.stream_id, storyId, "append"),
          timelineLoading: false,
        })),
      }));
    } catch (error) {
      await get().loadTimeline(storyId);
      throw error;
    }
  },
  submitGuide: async (storyId, note) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null }) }));
    const streamId = await timelineApi.submitGuide(storyId, note);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { streaming: newStream(streamId, storyId, "append") }) }));
  },
  continueScene: async (storyId) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null }) }));
    const streamId = await timelineApi.continueScene(storyId);
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { streaming: newStream(streamId, storyId, "append") }) }));
  },
  retryNarration: async (storyId, entryId) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null }) }));
    const result = await timelineApi.retry(storyId, entryId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, { streaming: newStream(result.stream_id, storyId, "replace", result.entry_id) }),
    }));
  },
  generateVariant: async (storyId, entryId) => {
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { turnError: null }) }));
    const streamId = await timelineApi.generateVariant(storyId, entryId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, { streaming: newStream(streamId, storyId, "replace", entryId) }),
    }));
  },
  eraseLastExchange: async (storyId) => {
    const ids = await timelineApi.eraseLastExchange(storyId);
    if (!ids.length) return;
    advanceGeneration(timelineGenerations, storyId);
    ids.forEach((entryId) => advanceGeneration(variantGenerations, variantKey(storyId, entryId)));
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => {
        const imagesByEntry = { ...bundle.imagesByEntry };
        const variantsByEntry = { ...bundle.variantsByEntry };
        const rollsByEntry = { ...bundle.rollsByEntry };
        const rollDetailByEntry = { ...bundle.rollDetailByEntry };
        ids.forEach((entryId) => {
          delete imagesByEntry[entryId];
          delete variantsByEntry[entryId];
          delete rollsByEntry[entryId];
          delete rollDetailByEntry[entryId];
        });
        return {
          entries: bundle.entries.filter((entry) => !ids.includes(entry.id)),
          imagesByEntry,
          variantsByEntry,
          rollsByEntry,
          rollDetailByEntry,
          imagePendingFor: bundle.imagePendingFor.filter((entryId) => !ids.includes(entryId)),
          timelineLoading: false,
        };
      }),
    }));
    await get().loadCharacters(storyId);
  },
  editEntry: async (storyId, entryId, content) => {
    const entry = await timelineApi.edit(entryId, content);
    advanceGeneration(timelineGenerations, storyId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        entries: replaceEntry(bundle.entries, entryId, entry),
        imagesByEntry: { ...bundle.imagesByEntry, [entryId]: [] },
        timelineLoading: false,
      })),
    }));
  },
  selectVariant: async (storyId, entryId, variantEntryId) => {
    const entry = await timelineApi.selectVariant(entryId, variantEntryId);
    advanceGeneration(timelineGenerations, storyId);
    advanceGeneration(variantGenerations, variantKey(storyId, entryId));
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        entries: replaceEntry(bundle.entries, entryId, entry),
        variantsByEntry: {
          ...bundle.variantsByEntry,
          [entryId]: (bundle.variantsByEntry[entryId] ?? []).map((variant) => ({
            ...variant,
            is_selected: variant.id === variantEntryId,
          })),
        },
        imagesByEntry: { ...bundle.imagesByEntry, [entryId]: [] },
        timelineLoading: false,
      })),
    }));
  },
  loadVariantsForEntry: async (storyId, entryId) => {
    const key = variantKey(storyId, entryId);
    const generation = advanceGeneration(variantGenerations, key);
    try {
      const variants = await timelineApi.listVariants(entryId);
      if (!isCurrentGeneration(variantGenerations, key, generation)) return;
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          variantsByEntry: { ...bundle.variantsByEntry, [entryId]: variants },
        })),
      }));
    } catch (error) {
      console.error("failed to load variants", error);
    }
  },
  loadImagesForStory: async (storyId) => {
    try {
      const images = await timelineApi.listImages(storyId);
      const grouped: Record<string, StoryImage[]> = {};
      images.forEach((image) => (grouped[image.entry_id] ??= []).push(image));
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          imagesByEntry: { ...bundle.imagesByEntry, ...grouped },
        })),
      }));
    } catch (error) {
      console.error("failed to load images", error);
    }
  },
  generateImageForEntry: async (storyId, entryId, hint) => {
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        imageError: null,
        imagePendingFor: bundle.imagePendingFor.includes(entryId) ? bundle.imagePendingFor : [...bundle.imagePendingFor, entryId],
      })),
    }));
    try {
      get()._imageGenerated(await timelineApi.generateImage(entryId, hint));
    } catch (error) {
      get()._imageFailed(entryId);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { imageError: String(error) }) }));
    }
  },
  loadRollsForStory: async (storyId) => {
    try {
      const rolls = await timelineApi.listRolls(storyId);
      const grouped: Record<string, RollDetail[]> = {};
      rolls.forEach((detail) => (grouped[detail.roll.entry_id] ??= []).push(detail));
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          rollsByEntry: { ...bundle.rollsByEntry, ...grouped },
        })),
      }));
    } catch (error) {
      console.error("failed to load rolls", error);
    }
  },
  loadRollDetail: async (storyId, entryId) => {
    try {
      const details = await dicerollApi.listRollDetailsForEntry(storyId, entryId);
      set((state) => ({
        bundles: patchBundle(state.bundles, storyId, (bundle) => ({
          rollDetailByEntry: { ...bundle.rollDetailByEntry, [entryId]: details },
        })),
      }));
    } catch (error) {
      console.error("failed to load roll detail", error);
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
    advanceGeneration(timelineGenerations, storyId);
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        entries: current.mode === "replace" && current.targetEntryId
          ? replaceEntry(bundle.entries, current.targetEntryId, payload.entry)
          : bundle.entries.some((entry) => entry.id === payload.entry.id)
            ? replaceEntry(bundle.entries, payload.entry.id, payload.entry)
            : [...bundle.entries, payload.entry],
        streaming: null,
        turnActivity: { thoughts: current.thoughts, tools: current.toolLog },
        timelineLoading: false,
        ...(current.mode === "replace" && current.targetEntryId
          ? { imagesByEntry: { ...bundle.imagesByEntry, [current.targetEntryId]: [] } }
          : {}),
      })),
    }));
    if (current.mode === "replace" && current.targetEntryId) {
      get().loadVariantsForEntry(storyId, current.targetEntryId);
    }
    get().loadRollsForStory(storyId);
    get().loadCharacters(storyId);
  },
  _swipeDone: (payload) => {
    const found = findStream(get().bundles, payload.stream_id);
    if (!found) return;
    const [storyId, current] = found;
    advanceGeneration(timelineGenerations, storyId);
    advanceGeneration(variantGenerations, variantKey(storyId, payload.entry.id));
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, (bundle) => ({
        entries: replaceEntry(bundle.entries, payload.entry.id, payload.entry),
        variantsByEntry: { ...bundle.variantsByEntry, [payload.entry.id]: payload.variants },
        imagesByEntry: { ...bundle.imagesByEntry, [payload.entry.id]: [] },
        streaming: null,
        turnActivity: { thoughts: current.thoughts, tools: current.toolLog },
        timelineLoading: false,
      })),
    }));
  },
  _fail: (streamId, message) => {
    const found = findStream(get().bundles, streamId);
    if (!found) return;
    const [storyId, current] = found;
    set((state) => ({
      bundles: patchBundle(state.bundles, storyId, {
        streaming: null,
        turnActivity: { thoughts: current.thoughts, tools: current.toolLog },
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

  loadDiceSettings: async (storyId) => {
    const activeLoad = settingsLoads.get(storyId);
    if (activeLoad) return activeLoad;
    set((state) => ({ bundles: patchBundle(state.bundles, storyId, { diceSettingsLoading: true }) }));
    const load = enqueueSettings(storyId, async () => {
      const diceSettings = await dicerollApi.getSettings(storyId);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { diceSettings }) }));
    })
      .catch((error) => console.error("failed to load dice-roll settings", error))
      .finally(() => {
        if (settingsLoads.get(storyId) === load) settingsLoads.delete(storyId);
        set((state) => ({ bundles: patchBundle(state.bundles, storyId, { diceSettingsLoading: false }) }));
      });
    settingsLoads.set(storyId, load);
    return load;
  },
  saveDiceSettings: async (storyId, patch) => {
    if (!storyId) {
      set((state) => ({
        draftDiceSettings: { ...(state.draftDiceSettings ?? DEFAULT_DICEROLL_SETTINGS), ...patch },
      }));
      return;
    }
    await enqueueSettings(storyId, async () => {
      const current = get().bundles[storyId]?.diceSettings ?? await dicerollApi.getSettings(storyId);
      const next = { ...current, ...patch };
      await dicerollApi.saveSettings(storyId, next.dice_mode, next.attributes_enabled, next.reasoning_effort);
      set((state) => ({ bundles: patchBundle(state.bundles, storyId, { diceSettings: next }) }));
    });
  },
  resetDraftDiceSettings: () => set({ draftDiceSettings: null }),

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
