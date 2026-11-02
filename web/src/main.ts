// The bare page: it holds the one-tab lock, starts the worker, sends it core's commands and shows
// the events it sends back. The real screens replace this.

import type { Event as ClientEvent, Command } from "./wasm/keryx_wasm";

/** Two tabs would each load the device's MLS state and break it, so only the lock's holder runs. */
const LOCK = "keryx";

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) {
    throw new Error(`#${id} is missing`);
  }
  return found as T;
}

const value = (id: string) => element<HTMLInputElement>(id).value.trim();
const log = element<HTMLDivElement>("log");
let worker: Worker | null = null;

function show(line: string) {
  const entry = document.createElement("div");
  entry.textContent = line;
  log.prepend(entry);
}

function shown(event: ClientEvent) {
  show(JSON.stringify(event));
  switch (event.type) {
    case "words":
      element<HTMLTextAreaElement>("words").value = event.words;
      break;
    case "search_results":
      element<HTMLInputElement>("account").value = event.account_list[0]?.id ?? "";
      break;
    case "chat_ready":
      element<HTMLInputElement>("chat").value = event.chat;
      break;
  }
}

function command(name: string): Command | null {
  const password = value("password");
  const words = element<HTMLTextAreaElement>("words").value.trim();
  switch (name) {
    case "sign_up":
      return { type: "sign_up", username: value("username"), password };
    case "log_in":
      return { type: "log_in", username: value("username"), password };
    case "log_out":
      return { type: "log_out" };
    case "generate_words":
      return { type: "generate_words", count: 12 };
    case "set_words":
      return { type: "set_words", words };
    case "restore":
      return { type: "restore", words };
    case "search":
      return { type: "search", text: value("search") };
    case "start_chat":
      return { type: "start_chat", account: value("account") };
    case "chat_list":
      return { type: "chat_list" };
    case "open_chat":
      return { type: "open_chat", chat: value("chat") };
    case "send_text":
      return { type: "send_text", chat: value("chat"), text: value("text") };
    case "devices":
      return { type: "devices" };
  }
  return null;
}

for (const button of document.querySelectorAll<HTMLButtonElement>("[data-command]")) {
  button.onclick = () => {
    const sent = command(button.dataset.command ?? "");
    if (sent && worker) {
      worker.postMessage(sent);
    }
  };
}

function run() {
  element("elsewhere").hidden = true;
  element("app").hidden = false;
  worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  worker.onmessage = (message: MessageEvent<ClientEvent>) => shown(message.data);
  worker.onerror = (error) => show(`The worker failed: ${error.message}`);
  // Held for as long as the page lives.
  return new Promise<void>(() => {});
}

/** Another tab took the lock: stop, since only one may hold the device's state. */
function elsewhere() {
  worker?.terminate();
  worker = null;
  element("app").hidden = true;
  element("elsewhere").hidden = false;
}

navigator.locks
  .request(LOCK, { ifAvailable: true }, (lock) => (lock ? run() : elsewhere()))
  .catch(elsewhere);

element<HTMLButtonElement>("use-here").onclick = () => {
  navigator.locks.request(LOCK, { steal: true }, run).catch(elsewhere);
};
