import { create } from "zustand";
import type { NarrationDonePayload, Passage, PassageVariant, RollDetail, StoryImage, SwipeDonePayload } from "../../shared/types";
import { passagesApi } from "./api";

interface StreamingState {
  streamId: string;
  branchId: string;
  text: string;
  thoughts: string;
  /** "append" adds a new trailing passage on completion (turn/guide/continue).
   *  "replace" shows the live buffer inside an existing passage's slot
   *  (retry/swipe) until that passage is replaced or updated in place. */
  mode: "append" | "replace";
  targetPassageId?: string;
}

interface StoryState {
  passagesByBranch: Record<string, Passage[]>;
  passagesLoading: boolean;
  streamingByBranch: Record<string, StreamingState>;
  turnError: string | null;

  variantsByPassage: Record<string, PassageVariant[]>;

  imagesByPassage: Record<string, StoryImage[]>;
  /** Passages with an image in flight — the player's own "See" and the
   *  narrator's own decision both land here, so both show a placeholder. */
  imagePendingFor: string[];
  imageError: string | null;

  /** Cheap summary (names, no attribute snapshots) for every roll in the branch. */
  rollByPassage: Record<string, RollDetail>;
  /** Full detail (attribute snapshots included), lazy-loaded on expand. */
  rollDetailByPassage: Record<string, RollDetail>;

  loadPassages: (branchId: string) => Promise<void>;
  submitStoryText: (branchId: string, content: string) => Promise<void>;
  submitTurn: (branchId: string, mode: "do" | "say", content: string) => Promise<void>;
  submitGuide: (branchId: string, note: string) => Promise<void>;
  continueScene: (branchId: string) => Promise<void>;
  retryPassage: (branchId: string, passageId: string) => Promise<void>;
  swipePassage: (branchId: string, passageId: string) => Promise<void>;
  eraseLastExchange: (branchId: string) => Promise<void>;
  editPassage: (branchId: string, passageId: string, content: string) => Promise<void>;
  switchVariant: (branchId: string, passageId: string, variantId: string) => Promise<void>;
  loadVariantsForPassage: (passageId: string) => Promise<void>;

  loadImagesForBranch: (branchId: string) => Promise<void>;
  generateImageForPassage: (passageId: string, promptHint?: string) => Promise<void>;

  loadRollsForBranch: (branchId: string) => Promise<void>;
  loadRollDetail: (passageId: string) => Promise<void>;

  _appendDelta: (streamId: string, text: string) => void;
  _appendThoughts: (streamId: string, text: string) => void;
  _finalize: (payload: NarrationDonePayload) => void;
  _swipeDone: (payload: SwipeDonePayload) => void;
  _fail: (streamId: string, message: string) => void;
  _imagePending: (passageId: string) => void;
  _imageGenerated: (image: StoryImage) => void;
  _imageFailed: (passageId: string) => void;
}

function replacePassage(passages: Passage[], id: string, next: Passage): Passage[] {
  const idx = passages.findIndex((p) => p.id === id);
  if (idx === -1) return passages;
  const copy = passages.slice();
  copy[idx] = next;
  return copy;
}

function findStream(
  streams: Record<string, StreamingState>,
  streamId: string,
): [string, StreamingState] | undefined {
  return Object.entries(streams).find(([, stream]) => stream.streamId === streamId);
}

function removeStream(streams: Record<string, StreamingState>, branchId: string) {
  const next = { ...streams };
  delete next[branchId];
  return next;
}

function removeOne(items: string[], value: string) {
  const index = items.indexOf(value);
  if (index === -1) return items;
  return items.slice(0, index).concat(items.slice(index + 1));
}

export const useStoryStore = create<StoryState>((set, get) => ({
  passagesByBranch: {},
  passagesLoading: false,
  streamingByBranch: {},
  turnError: null,

  variantsByPassage: {},

  imagesByPassage: {},
  imagePendingFor: [],
  imageError: null,

  rollByPassage: {},
  rollDetailByPassage: {},

  loadPassages: async (branchId: string) => {
    set({ passagesLoading: true });
    try {
      const passages = await passagesApi.list(branchId);
      set((s) => ({ passagesByBranch: { ...s.passagesByBranch, [branchId]: passages }, passagesLoading: false }));
    } catch (e) {
      console.error("failed to load passages", e);
      set({ passagesLoading: false });
    }
  },

  submitStoryText: async (branchId: string, content: string) => {
    set({ turnError: null });
    const result = await passagesApi.submitStory(branchId, content);
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: [...(s.passagesByBranch[branchId] ?? []), result.passage],
      },
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { streamId: result.stream_id, branchId, text: "", thoughts: "", mode: "append" },
      },
    }));
  },

  submitTurn: async (branchId: string, mode: "do" | "say", content: string) => {
    set({ turnError: null });
    const result = await passagesApi.submitTurn(branchId, mode, content);
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: [...(s.passagesByBranch[branchId] ?? []), result.passage],
      },
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { streamId: result.stream_id, branchId, text: "", thoughts: "", mode: "append" },
      },
    }));
  },

  submitGuide: async (branchId: string, note: string) => {
    set({ turnError: null });
    const streamId = await passagesApi.submitGuide(branchId, note);
    set((s) => ({
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { streamId, branchId, text: "", thoughts: "", mode: "append" },
      },
    }));
  },

  continueScene: async (branchId: string) => {
    set({ turnError: null });
    const streamId = await passagesApi.continueScene(branchId);
    set((s) => ({
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { streamId, branchId, text: "", thoughts: "", mode: "append" },
      },
    }));
  },

  retryPassage: async (branchId: string, passageId: string) => {
    set({ turnError: null });
    const result = await passagesApi.retry(branchId, passageId);
    set((s) => ({
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: {
          streamId: result.stream_id,
          branchId,
          text: "",
          thoughts: "",
          mode: "replace",
          targetPassageId: result.removed_passage_id,
        },
      },
    }));
  },

  swipePassage: async (branchId: string, passageId: string) => {
    set({ turnError: null });
    const streamId = await passagesApi.swipe(branchId, passageId);
    set((s) => ({
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { streamId, branchId, text: "", thoughts: "", mode: "replace", targetPassageId: passageId },
      },
    }));
  },

  eraseLastExchange: async (branchId: string) => {
    const removedIds = await passagesApi.eraseLastExchange(branchId);
    if (removedIds.length === 0) return;
    set((s) => {
      const imagesByPassage = { ...s.imagesByPassage };
      for (const id of removedIds) delete imagesByPassage[id];
      return {
        passagesByBranch: {
          ...s.passagesByBranch,
          [branchId]: (s.passagesByBranch[branchId] ?? []).filter((p) => !removedIds.includes(p.id)),
        },
        imagesByPassage,
        imagePendingFor: s.imagePendingFor.filter((id) => !removedIds.includes(id)),
      };
    });
  },

  editPassage: async (branchId: string, passageId: string, content: string) => {
    const passage = await passagesApi.edit(passageId, content);
    set((s) => ({
      passagesByBranch: { ...s.passagesByBranch, [branchId]: replacePassage(s.passagesByBranch[branchId] ?? [], passageId, passage) },
      imagesByPassage: { ...s.imagesByPassage, [passageId]: [] },
    }));
  },

  switchVariant: async (branchId: string, passageId: string, variantId: string) => {
    const passage = await passagesApi.switchVariant(passageId, variantId);
    set((s) => ({
      passagesByBranch: { ...s.passagesByBranch, [branchId]: replacePassage(s.passagesByBranch[branchId] ?? [], passageId, passage) },
      variantsByPassage: {
        ...s.variantsByPassage,
        [passageId]: (s.variantsByPassage[passageId] ?? []).map((v) => ({ ...v, is_selected: v.id === variantId })),
      },
      imagesByPassage: { ...s.imagesByPassage, [passageId]: [] },
    }));
  },

  loadVariantsForPassage: async (passageId: string) => {
    try {
      const variants = await passagesApi.listVariants(passageId);
      if (variants.length > 0) {
        set((s) => ({ variantsByPassage: { ...s.variantsByPassage, [passageId]: variants } }));
      }
    } catch (e) {
      console.error("failed to load variants", e);
    }
  },

  loadImagesForBranch: async (branchId: string) => {
    try {
      const images = await passagesApi.listImages(branchId);
      const grouped: Record<string, StoryImage[]> = {};
      for (const image of images) {
        (grouped[image.passage_id] ??= []).push(image);
      }
      set((s) => {
        const passageIds = new Set((s.passagesByBranch[branchId] ?? []).map((passage) => passage.id));
        const retained = Object.fromEntries(
          Object.entries(s.imagesByPassage).filter(([passageId]) => !passageIds.has(passageId)),
        );
        return { imagesByPassage: { ...retained, ...grouped } };
      });
    } catch (e) {
      console.error("failed to load images for branch", e);
    }
  },

  generateImageForPassage: async (passageId: string, promptHint?: string) => {
    set({ imageError: null });
    get()._imagePending(passageId);
    try {
      const image = await passagesApi.generateImage(passageId, promptHint);
      get()._imageGenerated(image);
    } catch (e) {
      get()._imageFailed(passageId);
      set({ imageError: String(e) });
    }
  },

  loadRollsForBranch: async (branchId: string) => {
    try {
      const rolls = await passagesApi.listRolls(branchId);
      const byPassage: Record<string, RollDetail> = {};
      for (const summary of rolls) byPassage[summary.roll.passage_id] = summary;
      set((s) => ({ rollByPassage: { ...s.rollByPassage, ...byPassage } }));
    } catch (e) {
      console.error("failed to load rolls for branch", e);
    }
  },

  loadRollDetail: async (passageId: string) => {
    try {
      const detail = await passagesApi.getRollDetail(passageId);
      if (detail) {
        set((s) => ({ rollDetailByPassage: { ...s.rollDetailByPassage, [passageId]: detail } }));
      }
    } catch (e) {
      console.error("failed to load roll detail", e);
    }
  },

  _appendDelta: (streamId, text) => {
    const found = findStream(get().streamingByBranch, streamId);
    if (!found) return;
    const [branchId, current] = found;
    set((s) => ({
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { ...current, text: current.text + text },
      },
    }));
  },

  _appendThoughts: (streamId, text) => {
    const found = findStream(get().streamingByBranch, streamId);
    if (!found) return;
    const [branchId, current] = found;
    set((s) => ({
      streamingByBranch: {
        ...s.streamingByBranch,
        [branchId]: { ...current, thoughts: current.thoughts + text },
      },
    }));
  },

  _finalize: (payload) => {
    const found = findStream(get().streamingByBranch, payload.stream_id);
    if (!found) return;
    const [, current] = found;
    const branchId = payload.passage.branch_id;
    set((s) => {
      const existing = s.passagesByBranch[branchId] ?? [];
      const next =
        current.mode === "replace" && current.targetPassageId
          ? existing.filter((p) => p.id !== current.targetPassageId).concat(payload.passage)
          : existing.concat(payload.passage);
      return {
        passagesByBranch: { ...s.passagesByBranch, [branchId]: next },
        streamingByBranch: removeStream(s.streamingByBranch, branchId),
        ...(current.mode === "replace" && current.targetPassageId
          ? { imagesByPassage: { ...s.imagesByPassage, [current.targetPassageId]: [] } }
          : {}),
      };
    });
    // A Stage 1/2 roll may have landed for the new passage — pick it up.
    get().loadRollsForBranch(branchId);
  },

  _swipeDone: (payload) => {
    const found = findStream(get().streamingByBranch, payload.stream_id);
    if (!found) return;
    const [, current] = found;
    const branchId = current.branchId;
    set((s) => ({
      passagesByBranch: {
        ...s.passagesByBranch,
        [branchId]: replacePassage(s.passagesByBranch[branchId] ?? [], payload.passage.id, payload.passage),
      },
      variantsByPassage: { ...s.variantsByPassage, [payload.passage.id]: payload.variants },
      imagesByPassage: { ...s.imagesByPassage, [payload.passage.id]: [] },
      streamingByBranch: removeStream(s.streamingByBranch, branchId),
    }));
  },

  _fail: (streamId, message) => {
    const found = findStream(get().streamingByBranch, streamId);
    if (!found) return;
    const [branchId] = found;
    set((s) => ({ streamingByBranch: removeStream(s.streamingByBranch, branchId), turnError: message }));
  },

  _imagePending: (passageId) => {
    set((s) => ({ imagePendingFor: [...s.imagePendingFor, passageId] }));
  },

  // Shared by the player's own "See" and the narrator's scene-image-generated
  // event, so an image lands the same way whoever asked for it.
  _imageGenerated: (image) => {
    set((s) => ({
      imagesByPassage: {
        ...s.imagesByPassage,
        [image.passage_id]: [...(s.imagesByPassage[image.passage_id] ?? []), image],
      },
      imagePendingFor: removeOne(s.imagePendingFor, image.passage_id),
    }));
  },

  _imageFailed: (passageId) => {
    set((s) => ({ imagePendingFor: removeOne(s.imagePendingFor, passageId) }));
  },
}));
