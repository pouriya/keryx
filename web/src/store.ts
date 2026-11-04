// The page's state. Memory only: words, passwords and tokens never go to storage from here.

import { create } from "zustand";
import type { Command, Event } from "./wasm/keryx_wasm";

export type Phase = "loading" | "account" | "setup" | "words" | "in";
export type Language = "en" | "fa";

export interface Failure {
  code: string;
  message: string;
}

export interface Mark {
  account: string;
  receivedSeq: number;
  seenSeq: number | null;
}

export interface ChatRow {
  id: string;
  type: number;
  lastAt: number | null;
  lastContentSeq: number | null;
  seenSeq: number;
  marks: Mark[];
  preview: string;
  previewAccount: string | null;
  unread: number;
}

export interface Person {
  username: string;
  displayName: string | null;
}

interface State {
  phase: Phase;
  held: boolean;
  language: Language;
  error: Failure | null;
  busy: boolean;
  username: string;
  /** Usernames saved on this browser, and the one to fill the field with. */
  saved: string[];
  remembered: string;
  /// `null` until `GET /api/v1/public/features` answers. Sign up stays hidden until it is true.
  registration: boolean | null;
  captcha: boolean;
  captchaId: string | null;
  captchaImage: string | null;
  registered: boolean;
  restore: boolean;
  words: string | null;
  me: string | null;
  rows: ChatRow[];
  people: Record<string, Person>;
  send: (command: Command) => void;
  useHere: () => void;
  setHeld: (held: boolean) => void;
  setSend: (send: (command: Command) => void) => void;
  setUseHere: (useHere: () => void) => void;
  forgetWords: () => void;
  clearError: () => void;
  setKnown: (usernames: string[], username: string | null) => void;
  apply: (event: Event) => void;
}

export const useStore = create<State>((set, get) => {
  let selfChat: string | null = null;
  let own: string | null = null;
  const names = new Map<string, string>();
  const rows = new Map<string, { row: ChatRow; counted: number[] }>();
  let order: string[] = [];
  const publishRows = () => {
    set({
      rows: order.flatMap((id) => {
        const held = rows.get(id);
        return held ? [{ ...held.row, marks: [...held.row.marks] }] : [];
      }),
    });
  };
  const named = (id: string | null) => {
    if (id === null || get().username !== "") {
      return;
    }
    const name = names.get(id);
    if (name !== undefined) {
      set({ username: name, phase: "in" });
    }
  };
  return {
    phase: "loading",
    held: false,
    language: navigator.language.toLowerCase().startsWith("fa") ? "fa" : "en",
    error: null,
    busy: false,
    username: "",
    saved: [],
    remembered: "",
    registration: null,
    captcha: false,
    captchaId: null,
    captchaImage: null,
    registered: false,
    restore: false,
    words: null,
    me: null,
    rows: [],
    people: {},
    send: () => {},
    useHere: () => {},
    setHeld: (held) => set({ held }),
    setSend: (send) => set({ send }),
    setUseHere: (useHere) => set({ useHere }),
    forgetWords: () => set({ words: null }),
    clearError: () => set({ error: null }),
    setKnown: (usernames, username) =>
      set({
        saved: usernames,
        remembered: username ?? get().remembered,
      }),
    apply: (event) => {
      switch (event.type) {
        case "features":
          set({ registration: event.registration, captcha: event.captcha });
          return;
        case "captcha":
          set({ captchaId: event.id, captchaImage: event.image });
          return;
        case "registered":
          set({ registered: true, captchaId: null, captchaImage: null, busy: false, error: null });
          return;
        case "need_login": {
          const setup = get().phase === "loading" && location.pathname === "/setup";
          set({
            phase: setup ? "setup" : "account",
            busy: false,
            error: null,
            words: null,
            registered: false,
          });
          return;
        }
        case "need_words":
          set({
            phase: "words",
            restore: event.restore,
            words: null,
            busy: false,
            error: null,
          });
          return;
        case "words":
          set({ words: event.words, busy: false });
          return;
        case "signed_in":
          set({
            username: event.username,
            me: event.account,
            busy: false,
            error: null,
            words: event.device === null ? get().words : null,
            phase: event.device === null ? get().phase : "in",
          });
          return;
        case "chats": {
          selfChat = event.chat_list.find((chat) => chat.type === 0)?.chat ?? selfChat;
          const keep = new Set<string>();
          order = [];
          for (const item of event.chat_list) {
            keep.add(item.chat);
            order.push(item.chat);
            const previous = rows.get(item.chat);
            const seenSeq = item.seen_seq ?? previous?.row.seenSeq ?? 0;
            let counted = previous?.counted ?? [];
            let unread = previous?.row.unread ?? 0;
            if (previous && seenSeq > previous.row.seenSeq) {
              counted = counted.filter((seq) => seq > seenSeq);
              unread = counted.length;
            }
            rows.set(item.chat, {
              counted,
              row: {
                id: item.chat,
                type: item.type,
                lastAt: item.last_at ?? previous?.row.lastAt ?? null,
                lastContentSeq: item.last_content_seq ?? previous?.row.lastContentSeq ?? null,
                seenSeq,
                marks: (item.mark_list ?? []).map((mark) => ({
                  account: mark.account,
                  receivedSeq: mark.received_seq,
                  seenSeq: mark.seen_seq ?? null,
                })),
                preview: previous?.row.preview ?? "",
                previewAccount: previous?.row.previewAccount ?? null,
                unread,
              },
            });
          }
          for (const id of rows.keys()) {
            if (!keep.has(id)) {
              rows.delete(id);
            }
          }
          publishRows();
          if (get().phase === "loading") {
            set({ phase: "in" });
          }
          return;
        }
        case "account":
          names.set(event.account.id, event.account.username);
          set({
            people: {
              ...get().people,
              [event.account.id]: {
                username: event.account.username,
                displayName: event.account.display_name ?? null,
              },
            },
          });
          if (own === event.account.id) {
            named(own);
          }
          return;
        case "message": {
          const message = event.message;
          if (message.chat === selfChat && message.account !== null) {
            own = message.account;
            named(own);
          }
          const held = rows.get(message.chat);
          if (!held) {
            return;
          }
          const content = message.content_kind != null;
          const row = { ...held.row };
          if (
            content &&
            message.content_kind !== 2 &&
            !message.mine &&
            message.seq > row.seenSeq &&
            !held.counted.includes(message.seq)
          ) {
            held.counted = [...held.counted, message.seq];
            row.unread += 1;
          }
          if (content && (row.lastContentSeq === null || message.seq >= row.lastContentSeq)) {
            const newer = row.lastContentSeq === null || message.seq > row.lastContentSeq;
            row.lastContentSeq = message.seq;
            row.preview = message.text ?? "";
            row.previewAccount = message.account;
            row.lastAt = message.saved_at;
            if (newer) {
              order = [message.chat, ...order.filter((id) => id !== message.chat)];
            }
          }
          held.row = row;
          publishRows();
          return;
        }
        case "marks": {
          const held = rows.get(event.chat);
          if (!held) {
            return;
          }
          const mark = event.mark;
          held.row = {
            ...held.row,
            marks: [
              ...held.row.marks.filter((item) => item.account !== mark.account),
              {
                account: mark.account,
                receivedSeq: mark.received_seq,
                seenSeq: mark.seen_seq ?? null,
              },
            ],
          };
          publishRows();
          return;
        }
        case "logged_out":
          selfChat = null;
          own = null;
          names.clear();
          rows.clear();
          order = [];
          set({
            phase: "account",
            busy: false,
            error: null,
            words: null,
            username: "",
            me: null,
            rows: [],
            people: {},
            registered: false,
            captchaId: null,
            captchaImage: null,
          });
          return;
        case "error":
          set({
            error: { code: event.code, message: event.message },
            busy: false,
            captchaId: get().captcha ? null : get().captchaId,
            captchaImage: get().captcha ? null : get().captchaImage,
          });
          return;
        case "device_limit":
          set({ error: { code: "device_limit", message: event.message }, busy: false });
          return;
        default:
          return;
      }
    },
  };
});

export function submit(command: Command) {
  useStore.setState({ busy: true, error: null });
  useStore.getState().send(command);
}
