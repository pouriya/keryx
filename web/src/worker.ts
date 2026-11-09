// The Web Worker: it holds the WASM, the keys, the MLS state, the queue and the WebSocket. It does
// what core's client asks, and feeds back what happened. Commands come from the page as they are,
// and events go back as they are. Nothing here logs, since what passes through holds secrets.

import { dropLegacy, lastUsername, openStore, remember, type Store, usernames } from "./storage";
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
const described = await platform();
let store: Store | null = null;
let bound: string | null = null;
let client = new Client("cookie", described);
let socket: WebSocket | null = null;
let timer: ReturnType<typeof setTimeout> | undefined;
let chain: Promise<void> = Promise.resolve();

function feed(input: Input) {
  for (const output of client.step(input, Date.now())) {
    run(output);
  }
}

function postKnown(names: string[], username: string | null) {
  postMessage({ type: "known", usernames: names, username });
}

/** Drops whatever device is in memory. Its database stays. */
function release() {
  socket?.close();
  socket = null;
  clearTimeout(timer);
  store?.close();
  store = null;
  bound = null;
  client = new Client("cookie", described);
}

/** Loads this username's own snapshot. A login does not connect until that login. */
async function bind(username: string, resume: boolean) {
  if (bound === username && store) {
    await remember(username);
    return;
  }
  const names = await usernames();
  if (!names.includes(username)) {
    release();
    return;
  }
  release();
  store = await openStore(username);
  bound = username;
  await remember(username);
  const [first, second] = await Promise.all(SLOTS.map((slot) => store?.get(slot) ?? null));
  feed({
    type: "loaded",
    first: first ?? null,
    second: second ?? null,
    resume,
  });
}

/** Points later saves at this username without loading another snapshot over the login. */
async function keep(username: string) {
  if (bound === username && store) {
    await remember(username);
    postKnown(await usernames(), username);
    return;
  }
  store?.close();
  store = await openStore(username);
  bound = username;
  await remember(username);
  postKnown(await usernames(), username);
}

async function handle(command: Command) {
  if (command.type === "log_in") {
    await bind(command.username, false);
  } else if (command.type === "sign_up" || command.type === "setup") {
    release();
  }
  feed({ type: "command", command });
}

function enqueue(command: Command) {
  chain = chain.then(() => handle(command)).catch(() => undefined);
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
      void chain.then(async () => {
        if (!store) {
          feed({ type: "save_failed", id });
          return;
        }
        try {
          await store.put(SLOTS[slot] ?? "slot0", bytes);
          feed({ type: "saved", id });
        } catch {
          feed({ type: "save_failed", id });
        }
      });
      break;
    }
    case "wipe":
      for (const slot of SLOTS) {
        void store?.remove(slot);
      }
      break;
    // The web keeps its database by username and loads it before a login, so a login that
    // reaches this has nothing kept for the account.
    case "open":
      feed({ type: "opened", first: null, second: null });
      break;
    case "wake_at":
      clearTimeout(timer);
      timer = setTimeout(() => feed({ type: "timer" }), Math.max(0, output.at - Date.now()));
      break;
    case "event":
      if (output.event.type === "signed_in") {
        const username = output.event.username;
        chain = chain.then(() => keep(username));
      }
      postMessage(output.event);
      break;
  }
}

await dropLegacy();
const names = await usernames();
const last = await lastUsername();
postKnown(names, last !== null && names.includes(last) ? last : null);
if (last !== null && names.includes(last)) {
  await bind(last, true);
} else {
  feed({ type: "loaded", first: null, second: null, resume: true });
}
for (const command of early.splice(0)) {
  enqueue(command);
}
self.onmessage = (message: MessageEvent<Command>) => enqueue(message.data);
addEventListener("online", () => enqueue({ type: "network_back" }));
