import { createRoot } from "react-dom/client";
import { App } from "./session";
import { useStore } from "./store";
import "./theme.css";
import type { Command, Event } from "./wasm/keryx_wasm";

const language = useStore.getState().language;
document.documentElement.lang = language;
document.documentElement.dir = language === "fa" ? "rtl" : "ltr";
document.title = language === "fa" ? "کریکس" : "keryx";

let worker: Worker | null = null;

function take(steal: boolean) {
  void navigator.locks
    .request("keryx", steal ? { steal: true } : { ifAvailable: true }, (lock) => {
      if (!lock) {
        useStore.getState().setHeld(true);
        return;
      }
      worker?.terminate();
      worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
      worker.onmessage = (message: MessageEvent<Event>) => {
        useStore.getState().apply(message.data);
      };
      worker.onerror = (error) => {
        useStore.setState({
          error: { code: "worker", message: error.message || "The page could not start." },
        });
      };
      useStore.getState().setSend((command: Command) => {
        worker?.postMessage(command);
      });
      useStore.getState().setHeld(false);
      return new Promise<void>(() => {});
    })
    .catch(() => {
      useStore.getState().setHeld(true);
    });
}

useStore.getState().setUseHere(() => take(true));

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing root");
}
take(false);
createRoot(root).render(<App />);
