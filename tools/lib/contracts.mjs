const REQUIRED_METHODS = {
  automation: ["commandFor", "execute"],
  collector: ["collect"],
  build: ["build"],
};

export function defineAdapter(kind, adapter) {
  if (!REQUIRED_METHODS[kind]) throw new Error(`Unknown adapter kind: ${kind}`);
  if (!adapter?.id || !Array.isArray(adapter.platforms)) throw new Error(`${kind} adapter requires id and platforms[]`);
  for (const method of REQUIRED_METHODS[kind]) {
    if (typeof adapter[method] !== "function") throw new Error(`${kind} adapter ${adapter.id} is missing ${method}()`);
  }
  return Object.freeze({ kind, ...adapter });
}

export class AdapterRegistry {
  #items = new Map();

  register(adapter) {
    const key = `${adapter.kind}:${adapter.id}`;
    if (this.#items.has(key)) throw new Error(`Adapter already registered: ${key}`);
    this.#items.set(key, adapter);
    return this;
  }

  get(kind, id, platform) {
    const adapter = this.#items.get(`${kind}:${id}`);
    if (!adapter) throw new Error(`Adapter not registered: ${kind}:${id}`);
    if (!adapter.platforms.includes(platform)) throw new Error(`${kind}:${id} does not support ${platform}`);
    return adapter;
  }

  list() {
    return [...this.#items.values()].map(({ kind, id, platforms }) => ({ kind, id, platforms }));
  }
}
