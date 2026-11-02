// The Web Worker: it holds the WASM, the keys, the MLS state, the queue and the WebSocket. It does
// what core's client asks, and feeds back what happened. Commands come from the page as they are,
// and events go back as they are. Nothing here logs, since what passes through holds secrets.

import { openStore } from "./storage";
import init, {
  Client,
  type Command,
  type Input,
  type Output,
  type Platform,
} from "./wasm/keryx_wasm";

interface Brand {
  brand: string;
  version: string;
}

interface UserAgentData {
  mobile: boolean;
  platform: string;
  brands: Brand[];
  getHighEntropyValues(hints: string[]): Promise<{ platformVersion?: string }>;
}

/** The operating system and its version in a user agent, checked in this order. */
const OS_PATTERNS: [string, RegExp][] = [
  ["android", /Android ([\d.]+)/],
  ["ios", /(?:iPhone|iPad|iPod).*? OS ([\d_]+)/],
  ["windows", /Windows NT ([\d.]+)/],
  ["macos", /Mac OS X ([\d_.]+)/],
  ["linux", /Linux/],
];

/** What the device says about itself: `userAgentData` where there is one, the user agent elsewhere. */
async function platform(): Promise<Platform> {
  const data = (navigator as Navigator & { userAgentData?: UserAgentData }).userAgentData;
  if (data) {
    const high = await data.getHighEntropyValues(["platformVersion"]);
    const brand =
      data.brands.find((each) => !/Not.?A.?Brand|Chromium/i.test(each.brand)) ?? data.brands[0];
    return {
      platform: data.mobile ? 1 : 0,
      os: data.platform.toLowerCase(),
      os_version: high.platformVersion ?? "",
      web_platform: brand?.brand.toLowerCase(),
      web_platform_version: brand?.version,
    };
  }
  const agent = navigator.userAgent;
  let os = "unknown";
  let osVersion = "";
  for (const [name, pattern] of OS_PATTERNS) {
    const found = agent.match(pattern);
    if (found) {
      os = name;
      osVersion = (found[1] ?? "").replaceAll("_", ".");
      break;
    }
  }
  const browser = agent.match(/(Firefox|Edg|OPR|Chrome)\/([\d.]+)/);
  const safari = agent.match(/Version\/([\d.]+).*Safari/);
  return {
    platform: /Mobi|Android|iPhone|iPad/.test(agent) ? 1 : 0,
    os,
    os_version: osVersion,
    web_platform: browser?.[1]?.toLowerCase() ?? (safari ? "safari" : undefined),
    web_platform_version: browser?.[2] ?? safari?.[1],
  };
}

const SLOTS = ["slot0", "slot1"];

// Commands sent while the WASM loads wait here, so none is lost.
const early: Command[] = [];
self.onmessage = (message: MessageEvent<Command>) => early.push(message.data);

await init();
const store = await openStore();
const client = new Client("cookie", await platform());
let socket: WebSocket | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;

function feed(input: Input) {
  for (const output of client.step(input, Date.now())) {
    run(output);
  }
}

function run(output: Output) {
  switch (output.type) {
    case "send":
      socket?.send(output.text);
      break;
    case "request": {
      const { id, method, path, body } = output;
      fetch(path, {
        method,
        credentials: "same-origin",
        headers: body === null ? {} : { "content-type": "application/json" },
        body: body ?? undefined,
      }).then(
        async (reply) =>
          feed({ type: "reply", id, status: reply.status, body: await reply.text() }),
        () => feed({ type: "reply", id, status: 0, body: "" }),
      );
      break;
    }
    case "connect": {
      socket?.close();
      const url = new URL("/api/v1/ws", location.href);
      url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
      const opened = new WebSocket(url, "keryx.v1");
      socket = opened;
      opened.onopen = () => socket === opened && feed({ type: "connected" });
      opened.onmessage = (message) =>
        socket === opened &&
        typeof message.data === "string" &&
        feed({ type: "frame", text: message.data });
      opened.onclose = (close) => {
        if (socket === opened) {
          socket = null;
          feed({ type: "closed", code: close.code, reason: close.reason });
        }
      };
      break;
    }
    case "disconnect": {
      const closing = socket;
      socket = null;
      closing?.close();
      break;
    }
    case "save": {
      const { id, slot, bytes } = output;
      store.put(SLOTS[slot] ?? "slot0", bytes).then(
        () => feed({ type: "saved", id }),
        () => feed({ type: "save_failed", id }),
      );
      break;
    }
    case "wipe":
      for (const slot of SLOTS) {
        store.remove(slot);
      }
      break;
    case "wake_at":
      clearTimeout(timer);
      timer = setTimeout(() => feed({ type: "timer" }), Math.max(0, output.at - Date.now()));
      break;
    case "event":
      postMessage(output.event);
      break;
  }
}

const [first, second] = await Promise.all(SLOTS.map((slot) => store.get(slot)));
feed({ type: "loaded", first: first ?? null, second: second ?? null });
for (const command of early.splice(0)) {
  feed({ type: "command", command });
}
self.onmessage = (message: MessageEvent<Command>) =>
  feed({ type: "command", command: message.data });
addEventListener("online", () => feed({ type: "command", command: { type: "network_back" } }));
