// Deux petits morceaux, sans bibliothèque :
// 1. rafraîchit les zones `data-live` sans recharger la page ni toucher aux formulaires ;
// 2. les gestes de la maquette : menus, recherche (Ctrl K), filtres, copie.
// Sans JavaScript, chaque page reste complète : il suffit de la recharger.
(() => {
  const root = document.querySelector("[data-poll]");

  const follows = () => document.querySelectorAll("[data-follow]");
  const wantsFollow = () => { const box = document.getElementById("log-follow"); return !box || box.checked; };
  const follow = () => { if (wantsFollow()) follows().forEach((el) => { el.scrollTop = el.scrollHeight; }); };
  follow();
  if (!root) return;

  let delay = Number(root.dataset.poll) || 8;
  let timer = null;

  const swap = (next) => {
    const current = document.querySelector(`[data-live="${next.dataset.live}"]`);
    if (!current) return;
    if (current.tagName === "DETAILS" && current.open) next.open = true;
    const pane = current.querySelector("[data-follow]");
    const atEnd = !pane || pane.scrollHeight - pane.scrollTop - pane.clientHeight < 24;
    current.replaceWith(next);
    document.dispatchEvent(new Event("live-swapped"));
    if (atEnd) follow();
  };

  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(refresh, delay * 1000);
  };

  async function refresh() {
    if (document.hidden) return schedule();
    try {
      const response = await fetch(location.href, { credentials: "same-origin", cache: "no-store" });
      // Une session expirée renvoie vers la connexion : on arrête de sonder.
      if (!response.ok || response.redirected) return;
      const doc = new DOMParser().parseFromString(await response.text(), "text/html");
      doc.querySelectorAll("[data-live]").forEach(swap);
      const poll = doc.querySelector("[data-poll]");
      if (poll) delay = Number(poll.dataset.poll) || delay;
    } catch (_) {
      /* réseau coupé : on réessaie au prochain tour */
    }
    schedule();
  }

  document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
  schedule();
})();

(() => {
  const $ = (selector, scope = document) => scope.querySelector(selector);
  const $$ = (selector, scope = document) => [...scope.querySelectorAll(selector)];

  /* ---- menus : un seul ouvert à la fois, fermés par un clic ailleurs ---- */
  document.addEventListener("click", (e) => {
    $$("details.menu[open]").forEach((d) => { if (!d.contains(e.target)) d.open = false; });
  });

  /* ---- copie ---- */
  const toast = (text) => {
    let box = $("#toasts");
    if (!box) { box = document.createElement("div"); box.id = "toasts"; box.className = "toasts"; box.setAttribute("aria-live", "polite"); document.body.append(box); }
    const el = document.createElement("div");
    el.className = "toast";
    el.setAttribute("role", "status");
    el.textContent = text;
    box.append(el);
    setTimeout(() => el.remove(), 4200);
  };
  const copy = async (text) => {
    try { await navigator.clipboard.writeText(text); toast("Copied to the clipboard"); }
    catch (_) { toast("Copy is not available here. Select the text instead."); }
  };

  /* ---- recherche (Ctrl K ou /) ---- */
  const palette = $("#palette");
  let index = 0;
  const visible = () => $$(".palette-item", palette).filter((a) => !a.hidden);
  const mark = () => {
    const items = visible();
    index = Math.max(0, Math.min(index, items.length - 1));
    $$(".palette-item", palette).forEach((a) => a.setAttribute("aria-selected", "false"));
    if (items[index]) { items[index].setAttribute("aria-selected", "true"); items[index].scrollIntoView({ block: "nearest" }); }
  };
  const filter = (term) => {
    const t = term.trim().toLowerCase();
    $$(".palette-item", palette).forEach((a) => { a.hidden = !!t && !a.dataset.q.includes(t); });
    $$(".palette-group", palette).forEach((g) => {
      if (g.id === "palette-none") return;
      let next = g.nextElementSibling;
      let any = false;
      while (next && !next.classList.contains("palette-group")) { if (!next.hidden) any = true; next = next.nextElementSibling; }
      g.hidden = !any;
    });
    $("#palette-none", palette).hidden = visible().length > 0;
    index = 0;
    mark();
  };
  const openPalette = () => {
    if (!palette || palette.open) return;
    $("#palette-q", palette).value = "";
    filter("");
    palette.showModal();
    $("#palette-q", palette).focus();
  };
  if (palette) {
    $("#palette-q", palette).addEventListener("input", (e) => filter(e.target.value));
    palette.addEventListener("click", (e) => { if (e.target === palette) palette.close(); });
    palette.addEventListener("keydown", (e) => {
      const items = visible();
      if (e.key === "ArrowDown") { e.preventDefault(); index = Math.min(index + 1, items.length - 1); mark(); }
      if (e.key === "ArrowUp") { e.preventDefault(); index = Math.max(index - 1, 0); mark(); }
      if (e.key === "Enter" && items[index]) { e.preventDefault(); location.href = items[index].href; }
    });
  }
  document.addEventListener("keydown", (e) => {
    const typing = e.target.matches("input, textarea, select");
    if ((e.key === "k" && (e.ctrlKey || e.metaKey)) || (e.key === "/" && !typing)) { e.preventDefault(); openPalette(); }
  });

  /* ---- clics ---- */
  document.addEventListener("click", (e) => {
    const t = e.target.closest("[data-action]");
    if (!t) return;
    if (t.dataset.action === "palette") openPalette();
    if (t.dataset.action === "copy") copy(t.dataset.text || "");
    if (t.dataset.action === "copy-log") copy($$(".logs-body .ln .tx").map((l) => l.textContent).join("\n"));
  });

  /* ---- formulaires qui s'envoient d'eux-mêmes (interrupteurs, listes) ---- */
  document.addEventListener("change", (e) => {
    const t = e.target;
    if (t.matches("[data-autosubmit]") && t.form) t.form.requestSubmit();
    if (t.matches('input[type="file"]')) {
      const out = $(`#${t.id}-name`);
      const f = t.files[0];
      if (!out || !f) return;
      if (!/^image\/(png|jpeg|gif)$/.test(f.type)) { out.textContent = "Only PNG, JPEG and GIF are accepted."; t.value = ""; return; }
      out.textContent = f.name;
    }
  });

  /* ---- filtres à la frappe ---- */
  const logFilter = () => {
    const box = $("#log-filter");
    if (!box) return;
    const t = box.value.trim().toLowerCase();
    $$(".logs-body .ln").forEach((l) => { l.hidden = !!t && !l.textContent.toLowerCase().includes(t); });
  };
  document.addEventListener("live-swapped", logFilter);
  document.addEventListener("input", (e) => {
    const t = e.target;
    if (t.id === "proj-q") $$(".project").forEach((c) => { c.hidden = !c.textContent.toLowerCase().includes(t.value.trim().toLowerCase()); });
    if (t.id === "market-q") {
      const q = t.value.trim().toLowerCase();
      const cards = $$("#market-results .bot-card");
      cards.forEach((c) => { c.hidden = !c.textContent.toLowerCase().includes(q); });
      const shown = cards.filter((c) => !c.hidden).length;
      const count = $("#market-count");
      if (count) count.textContent = `${q ? "Results" : "All bots"} (${shown})`;
      const empty = $("#market-empty");
      if (empty) empty.hidden = shown > 0 || !cards.length;
    }
    if (t.id === "log-filter") logFilter();
    if (t.id === "p-nick") { const n = $("#pv-profile .pf-name span"); if (n) n.textContent = t.value.trim() || t.placeholder; }
    if (t.id === "p-bio") {
      const b = $("#pv-profile .pf-bio"); if (b) b.textContent = t.value;
      const c = $("#p-bio-count"); if (c) c.textContent = `${t.value.length} / ${t.maxLength}`;
    }
  });
  const follow = $("#log-follow");
  if (follow) follow.addEventListener("change", () => { if (follow.checked) $$("[data-follow]").forEach((el) => { el.scrollTop = el.scrollHeight; }); });
  try {
    const banner = $("#cookie-banner");
    if (banner) {
      if (localStorage.getItem("cookie-ok") === "1") { banner.classList.add("hidden"); }
      const btn = banner.querySelector("button");
      if (btn) btn.addEventListener("click", () => { try { localStorage.setItem("cookie-ok", "1"); } catch (_) {} banner.classList.add("hidden"); });
    }
  } catch (_) {}
})();
