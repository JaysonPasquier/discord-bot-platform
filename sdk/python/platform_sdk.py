"""SDK Uplugit pour Python — vérifie si un serveur Discord a accès au premium d'un bot.

Aucune dépendance externe : uniquement la bibliothèque standard.

Usage synchrone :
    from platform_sdk import PlatformClient
    client = PlatformClient(base_url="https://api.uplugit.gg", token="tok_...")
    if client.is_premium(guild_id=1234567890):
        # accès premium accordé
        pass

Usage asynchrone (Python 3.9+) :
    import asyncio
    result = await client.async_is_premium(guild_id=1234567890)
"""

import asyncio
import json
import random
import threading
import time
import urllib.error
import urllib.request
from typing import Any, Callable, Optional, Union

DAY_MS = 24 * 60 * 60 * 1000


class PlatformAuthError(Exception):
    """Le jeton API a été refusé par la plateforme."""


class PlatformRequestError(Exception):
    """La plateforme a renvoyé une erreur inattendue."""
    def __init__(self, status: int) -> None:
        super().__init__(f"unexpected status {status}")
        self.status = status


class PlatformClient:
    """Client pour l'API premium d'Uplugit.

    Thread-safe : une seule instance peut être partagée entre plusieurs threads.
    """

    def __init__(
        self,
        *,
        base_url: str,
        token: str,
        refresh_ms: int = 60_000,
        cache_ttl_ms: int = DAY_MS,
        timeout_s: float = 3.0,
        default_on_failure: bool = False,
        # Injectables pour les tests.
        _fetch: Optional[Callable[[str, str], Any]] = None,
        _now: Optional[Callable[[], int]] = None,
    ) -> None:
        if not base_url or not token:
            raise TypeError("base_url and token are required")
        self._base_url = base_url.rstrip("/")
        self._token = token
        self._refresh_ms = refresh_ms
        self._cache_ttl_ms = cache_ttl_ms
        self._timeout_s = timeout_s
        self._default_on_failure = default_on_failure
        self._fetch = _fetch or self._http_fetch
        self._now = _now or (lambda: int(time.monotonic() * 1000))
        # Jitter par instance pour éviter la synchronisation des refreshs.
        self._jitter_ms = random.randint(0, min(refresh_ms, 15_000))
        # cache : guild_id (str) → (access: bool, at_ms: int)
        self._cache: dict[str, tuple[bool, int]] = {}
        self._lock = threading.Lock()

    def _http_fetch(self, guild_id: str, token: str) -> Any:
        """Appel HTTP réel vers l'API. Renvoie le corps JSON parsé."""
        url = f"{self._base_url}/v1/premium/{guild_id}"
        req = urllib.request.Request(
            url,
            headers={"Authorization": f"Bearer {token}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=self._timeout_s) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as exc:
            # On re-lève pour que is_premium puisse discriminer.
            raise exc
        except (urllib.error.URLError, OSError, json.JSONDecodeError):
            return None  # panne réseau ou JSON invalide → fallback

    def is_premium(self, guild_id: Union[int, str]) -> bool:
        """Vérifie si le serveur `guild_id` a accès au premium.

        Lève `PlatformAuthError` si le jeton est refusé (401).
        Lève `PlatformRequestError` pour tout autre code d'erreur inattendu.
        Retourne `default_on_failure` en cas de panne serveur ou réseau.
        """
        guild_id = _normalize(guild_id)
        now = self._now()

        with self._lock:
            cached = self._cache.get(guild_id)

        if cached is not None:
            access, at = cached
            if now - at < self._refresh_ms + self._jitter_ms:
                return access

        try:
            body = self._fetch(guild_id, self._token)
        except urllib.error.HTTPError as exc:
            if exc.code == 401:
                raise PlatformAuthError("the platform rejected the API token") from exc
            if exc.code >= 500:
                return self._fallback(cached, now)
            raise PlatformRequestError(exc.code) from exc
        except Exception:
            return self._fallback(cached, now)

        if body is None:
            return self._fallback(cached, now)

        access_val = body.get("access")
        if not isinstance(access_val, bool):
            return self._fallback(cached, now)

        with self._lock:
            self._cache[guild_id] = (access_val, now)
        return access_val

    async def async_is_premium(self, guild_id: Union[int, str]) -> bool:
        """Version asynchrone de `is_premium`. Exécute l'appel dans un thread."""
        return await asyncio.to_thread(self.is_premium, guild_id)

    def _fallback(self, cached: Optional[tuple[bool, int]], now: int) -> bool:
        if cached is not None:
            access, at = cached
            if now - at <= self._cache_ttl_ms:
                return access
        return self._default_on_failure


def _normalize(guild_id: Union[int, str]) -> str:
    if isinstance(guild_id, int) and guild_id >= 0:
        return str(guild_id)
    if isinstance(guild_id, str) and guild_id.isdigit() and 1 <= len(guild_id) <= 20:
        return guild_id
    raise TypeError(
        "guild_id must be a non-negative int or a decimal string of 1–20 digits"
    )
