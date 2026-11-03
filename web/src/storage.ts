// One IndexedDB database per username, one store, get and put by key. A put resolves only once
// its transaction has committed, so `saved` is never reported early. A database with no username
// is deleted and never read.

const LEGACY = "keryx";
const CURRENT = "keryx.current";
const PREFIX = "keryx:";
const STORE = "keryx";
const LAST = "username";

export interface Store {
  get(key: string): Promise<Uint8Array | null>;
  put(key: string, value: Uint8Array): Promise<void>;
  remove(key: string): Promise<void>;
  close(): void;
}

function done(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error);
  });
}

function removeDatabase(name: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.deleteDatabase(name);
    request.onsuccess = () => resolve();
    request.onerror = () => reject(request.error);
    request.onblocked = () => resolve();
  });
}

function openNamed(name: string): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(name, 1);
    request.onupgradeneeded = () => {
      if (!request.result.objectStoreNames.contains(STORE)) {
        request.result.createObjectStore(STORE);
      }
    };
    request.onerror = () => reject(request.error);
    request.onsuccess = () => resolve(request.result);
  });
}

function wrap(db: IDBDatabase): Store {
  const write = (change: (store: IDBObjectStore) => void) => {
    const transaction = db.transaction(STORE, "readwrite", { durability: "strict" });
    change(transaction.objectStore(STORE));
    return done(transaction);
  };
  return {
    get: (key) =>
      new Promise((resolve, reject) => {
        const get = db.transaction(STORE, "readonly").objectStore(STORE).get(key);
        get.onsuccess = () => resolve(get.result instanceof Uint8Array ? get.result : null);
        get.onerror = () => reject(get.error);
      }),
    put: (key, value) => write((store) => store.put(value, key)),
    remove: (key) => write((store) => store.delete(key)),
    close: () => db.close(),
  };
}

function isUsername(name: string): boolean {
  return /^[a-z_][a-z0-9_]{0,31}$/.test(name);
}

/** Deletes the old database that held a device and no username, and any name that is not a username. */
export async function dropLegacy(): Promise<void> {
  await removeDatabase(LEGACY);
  if (typeof indexedDB.databases !== "function") {
    return;
  }
  const listed = await indexedDB.databases();
  for (const database of listed) {
    const name = database.name ?? "";
    if (name.startsWith(PREFIX) && !isUsername(name.slice(PREFIX.length))) {
      await removeDatabase(name);
    }
  }
}

export async function usernames(): Promise<string[]> {
  if (typeof indexedDB.databases !== "function") {
    return [];
  }
  const listed = await indexedDB.databases();
  const names = listed.flatMap((database) => {
    const name = database.name ?? "";
    return name.startsWith(PREFIX) ? [name.slice(PREFIX.length)] : [];
  });
  names.sort();
  return names;
}

export async function lastUsername(): Promise<string | null> {
  const db = await openNamed(CURRENT);
  try {
    const value = await new Promise<unknown>((resolve, reject) => {
      const get = db.transaction(STORE, "readonly").objectStore(STORE).get(LAST);
      get.onsuccess = () => resolve(get.result);
      get.onerror = () => reject(get.error);
    });
    return typeof value === "string" && value !== "" ? value : null;
  } finally {
    db.close();
  }
}

export async function remember(username: string): Promise<void> {
  const db = await openNamed(CURRENT);
  try {
    const transaction = db.transaction(STORE, "readwrite", { durability: "strict" });
    transaction.objectStore(STORE).put(username, LAST);
    await done(transaction);
  } finally {
    db.close();
  }
}

/** Opens that username's database, creating it when this account is first saved. */
export async function openStore(username: string): Promise<Store> {
  if (!isUsername(username)) {
    throw new Error("That is not a username");
  }
  return wrap(await openNamed(PREFIX + username));
}
