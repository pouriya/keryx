// The device's two snapshot slots in IndexedDB: one database, one store, get and put by key. A put
// resolves only once its transaction has committed, so `saved` is never reported early.

const NAME = "keryx";

export interface Store {
  get(key: string): Promise<Uint8Array | null>;
  put(key: string, value: Uint8Array): Promise<void>;
  remove(key: string): Promise<void>;
}

function done(transaction: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    transaction.oncomplete = () => resolve();
    transaction.onerror = () => reject(transaction.error);
    transaction.onabort = () => reject(transaction.error);
  });
}

export function openStore(): Promise<Store> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(NAME, 1);
    request.onupgradeneeded = () => request.result.createObjectStore(NAME);
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      const write = (change: (store: IDBObjectStore) => void) => {
        const transaction = db.transaction(NAME, "readwrite", { durability: "strict" });
        change(transaction.objectStore(NAME));
        return done(transaction);
      };
      resolve({
        get: (key) =>
          new Promise((resolve, reject) => {
            const get = db.transaction(NAME, "readonly").objectStore(NAME).get(key);
            get.onsuccess = () => resolve(get.result instanceof Uint8Array ? get.result : null);
            get.onerror = () => reject(get.error);
          }),
        put: (key, value) => write((store) => store.put(value, key)),
        remove: (key) => write((store) => store.delete(key)),
      });
    };
  });
}
