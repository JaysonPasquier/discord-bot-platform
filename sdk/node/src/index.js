const DAY_MS = 24 * 60 * 60 * 1000;

export class PlatformAuthError extends Error {}
export class PlatformRequestError extends Error {}

// Un identifiant Discord dépasse 2^53 : un nombre JavaScript l'arrondirait en silence.
function normalizeGuildId(guildId) {
  if (typeof guildId === 'bigint' && guildId >= 0n) return guildId.toString();
  if (typeof guildId === 'string' && /^[0-9]{1,20}$/.test(guildId)) return guildId;
  throw new TypeError('guildId must be a decimal string or a non-negative bigint');
}

export class PlatformClient {
  #baseUrl;
  #token;
  #fetch;
  #now;
  #refreshMs;
  #cacheTtlMs;
  #timeoutMs;
  #defaultOnFailure;
  #jitter;
  #cache = new Map();

  constructor({
    baseUrl,
    token,
    fetch = globalThis.fetch,
    now = Date.now,
    refreshMs = 60_000,
    cacheTtlMs = DAY_MS,
    timeoutMs = 3_000,
    defaultOnFailure = false,
  } = {}) {
    if (!baseUrl || !token) throw new TypeError('baseUrl and token are required');
    this.#baseUrl = baseUrl.replace(/\/+$/, '');
    this.#token = token;
    this.#fetch = fetch;
    this.#now = now;
    this.#refreshMs = refreshMs;
    this.#cacheTtlMs = cacheTtlMs;
    this.#timeoutMs = timeoutMs;
    this.#defaultOnFailure = defaultOnFailure;
    // Jitter per-instance pour éviter que toutes les instances se synchronisent sur le même tick.
    this.#jitter = Math.floor(Math.random() * Math.min(refreshMs, 15_000));
  }

  async isPremium(guildId) {
    const id = normalizeGuildId(guildId);
    const now = this.#now();
    const cached = this.#cache.get(id);
    if (cached && now - cached.at < this.#refreshMs + this.#jitter) return cached.access;

    let response;
    try {
      response = await this.#fetch(`${this.#baseUrl}/v1/premium/${id}`, {
        headers: { authorization: `Bearer ${this.#token}` },
        signal: AbortSignal.timeout(this.#timeoutMs),
      });
    } catch {
      return this.#fallback(cached, now);
    }
    // Un jeton refusé n'est pas une panne : on ne le masque pas avec le cache.
    if (response.status === 401) throw new PlatformAuthError('the platform rejected the API token');
    if (response.status >= 500) return this.#fallback(cached, now);
    if (!response.ok) throw new PlatformRequestError(`unexpected status ${response.status}`);

    let body;
    try {
      body = await response.json();
    } catch {
      return this.#fallback(cached, now);
    }
    if (typeof body?.access !== 'boolean') return this.#fallback(cached, now);
    this.#cache.set(id, { access: body.access, at: now });
    return body.access;
  }

  #fallback(cached, now) {
    if (cached && now - cached.at <= this.#cacheTtlMs) return cached.access;
    return this.#defaultOnFailure;
  }
}
