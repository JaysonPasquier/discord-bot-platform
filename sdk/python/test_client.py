"""Tests du SDK Python. Exécution : python -m pytest test_client.py ou python test_client.py"""

import sys
import threading
import urllib.error

sys.path.insert(0, ".")

from platform_sdk import PlatformAuthError, PlatformClient, PlatformRequestError


def _ok(access: bool) -> dict:
    return {"guild_id": "1", "status": "active" if access else "expired", "access": access}


def _make(responses, *, default_on_failure=False, refresh_ms=60_000, cache_ttl_ms=86_400_000):
    queue = list(responses)
    t = {"ms": 1_000_000}

    def fetch(guild_id, token):
        if not queue:
            raise AssertionError("unexpected fetch call")
        item = queue.pop(0)
        if isinstance(item, Exception):
            raise item
        if isinstance(item, dict):
            return item
        raise urllib.error.HTTPError(url="", code=item, msg="", hdrs=None, fp=None)

    client = PlatformClient(
        base_url="http://api.test",
        token="tok",
        refresh_ms=refresh_ms,
        cache_ttl_ms=cache_ttl_ms,
        default_on_failure=default_on_failure,
        _fetch=fetch,
        _now=lambda: t["ms"],
    )
    return client, t, queue


def test_active_subscription_returns_true():
    client, _, _ = _make([_ok(True)])
    assert client.is_premium("1") is True


def test_no_subscription_returns_false():
    client, _, _ = _make([_ok(False)])
    assert client.is_premium("1") is False


def test_cache_avoids_second_request():
    client, t, _ = _make([_ok(True), _ok(False)])
    assert client.is_premium("1") is True
    t["ms"] += 59_999
    assert client.is_premium("1") is True  # cache hit


def test_cache_expires_after_refresh():
    client, t, _ = _make([_ok(True), _ok(False)])
    assert client.is_premium("1") is True
    t["ms"] += 60_000 + client._jitter_ms + 1
    assert client.is_premium("1") is False  # refresh


def test_guilds_have_independent_caches():
    client, _, _ = _make([_ok(True), _ok(False)])
    assert client.is_premium("1") is True
    assert client.is_premium("2") is False


def test_401_raises_auth_error():
    client, _, _ = _make([401])
    try:
        client.is_premium("1")
        assert False, "expected PlatformAuthError"
    except PlatformAuthError:
        pass


def test_500_falls_back_to_default():
    client, _, _ = _make([500], default_on_failure=True)
    assert client.is_premium("1") is True


def test_network_error_falls_back_to_cache():
    client, t, _ = _make([_ok(True), OSError("offline")], cache_ttl_ms=86_400_000)
    client.is_premium("1")
    t["ms"] += 120_000
    assert client.is_premium("1") is True  # stale cache within TTL


def test_cache_expired_returns_default_on_failure():
    client, t, _ = _make([_ok(True), 503], default_on_failure=False, cache_ttl_ms=100)
    client.is_premium("1")
    t["ms"] += 200  # cache TTL expired
    assert client.is_premium("1") is False


def test_other_client_errors_raise():
    client, _, _ = _make([400])
    try:
        client.is_premium("1")
        assert False, "expected PlatformRequestError"
    except PlatformRequestError as exc:
        assert exc.status == 400


def test_invalid_guild_id_raises_type_error():
    client, _, _ = _make([])
    for bad in [-1, 1.5, "", "abc", "1" * 21, None]:
        try:
            client.is_premium(bad)
            assert False, f"expected TypeError for {bad!r}"
        except TypeError:
            pass


def test_thread_safety():
    results = []
    client, _, _ = _make([_ok(True)] * 20)
    def run():
        try:
            results.append(client.is_premium("1"))
        except Exception as exc:
            results.append(exc)
    threads = [threading.Thread(target=run) for _ in range(10)]
    for th in threads:
        th.start()
    for th in threads:
        th.join()
    assert all(r is True for r in results), results


if __name__ == "__main__":
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    failed = 0
    for fn in tests:
        try:
            fn()
            print(f"  ok  {fn.__name__}")
        except Exception as exc:
            print(f"FAIL  {fn.__name__}: {exc}")
            failed += 1
    print(f"\n{len(tests) - failed}/{len(tests)} passed")
    sys.exit(1 if failed else 0)
