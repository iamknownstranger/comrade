/* ============================================================================
 * Comrade desktop frontend
 *
 * A no-build, vanilla-JS SPA that drives the Tauri "Command & Event Bridge".
 * `withGlobalTauri: true` (tauri.conf.json) exposes window.__TAURI__, so we use
 * window.__TAURI__.core.invoke + window.__TAURI__.event.listen directly.
 *
 * Progressive disclosure:
 *   vault door  ->  base workspace (Sabha | Vault)  ->  modality overlays
 *                                                       (Travel mesh / Couple)
 *
 * Every backend call goes through safeInvoke(), which surfaces errors as toasts
 * (Milestone 5) instead of failing silently in the console.
 * ========================================================================== */

(() => {
  "use strict";

  const STORE_PATH = "comrade-data";
  const EVENT_CHANNEL = "comrade://event";

  // ── Backend access (real Tauri, or a dev mock for browser preview) ────────
  const TAURI = window.__TAURI__;
  const hasTauri = !!(TAURI && TAURI.core && TAURI.event);
  const backend = hasTauri
    ? {
        invoke: (cmd, args) => TAURI.core.invoke(cmd, args),
        listen: (event, cb) => TAURI.event.listen(event, cb),
      }
    : mockBackend();

  // ── Tiny DOM helpers ──────────────────────────────────────────────────────
  const $ = (sel) => document.querySelector(sel);

  /** Build an element. Dynamic text always goes through textContent (no XSS). */
  function el(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (v == null) continue;
      if (k === "class") node.className = v;
      else if (k === "text") node.textContent = v;
      else if (k.startsWith("on") && typeof v === "function")
        node.addEventListener(k.slice(2).toLowerCase(), v);
      else node.setAttribute(k, v);
    }
    for (const c of children.flat()) {
      if (c == null) continue;
      node.append(c.nodeType ? c : document.createTextNode(String(c)));
    }
    return node;
  }

  const nowSecs = () => Math.floor(Date.now() / 1000);

  /**
   * Resolve a stable on-disk location for the encrypted vault. In a packaged
   * app the cwd is unpredictable, so prefer the OS app-data dir when Tauri's
   * path API is exposed; otherwise fall back to a cwd-relative folder.
   */
  async function resolveStorePath() {
    try {
      if (TAURI && TAURI.path && TAURI.path.appDataDir && TAURI.path.join) {
        const base = await TAURI.path.appDataDir();
        return await TAURI.path.join(base, "comrade-vault");
      }
    } catch {
      /* fall through to the relative default */
    }
    return STORE_PATH;
  }

  function shortNpub(s) {
    s = String(s || "");
    return s.length > 18 ? `${s.slice(0, 11)}…${s.slice(-5)}` : s;
  }

  function relTime(secs) {
    if (!secs) return "just now";
    const d = nowSecs() - Number(secs);
    if (d < 45) return "just now";
    if (d < 3600) return `${Math.floor(d / 60)}m ago`;
    if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
    return new Date(Number(secs) * 1000).toLocaleString();
  }

  function errText(e) {
    if (typeof e === "string") return e;
    if (e && e.message) return e.message;
    try {
      return JSON.stringify(e);
    } catch {
      return String(e);
    }
  }

  function debounce(fn, ms) {
    let t;
    return (...a) => {
      clearTimeout(t);
      t = setTimeout(() => fn(...a), ms);
    };
  }

  function setBusy(btn, busy) {
    if (!btn) return;
    btn.disabled = busy;
    btn.classList.toggle("is-busy", busy);
    const sp = btn.querySelector(".spinner");
    if (sp) sp.hidden = !busy;
  }

  // ── Toasts (Milestone 5) ──────────────────────────────────────────────────
  function showToast(message, type = "info", title) {
    const icons = { error: "⛔", success: "✓", info: "ℹ", warn: "⚠" };
    const toast = el(
      "div",
      { class: `toast ${type}`, role: "status" },
      el("span", { class: "toast-icon", text: icons[type] || "ℹ" }),
      el(
        "div",
        { class: "toast-body" },
        title ? el("div", { class: "toast-title", text: title }) : null,
        el("div", { text: message }),
      ),
    );
    $("#toasts").append(toast);
    const ttl = type === "error" ? 6500 : 3500;
    setTimeout(() => {
      toast.classList.add("leaving");
      setTimeout(() => toast.remove(), 250);
    }, ttl);
  }

  /** Single funnel for IPC: try/catch with an error toast, then rethrow. */
  async function safeInvoke(cmd, args, opts = {}) {
    try {
      return await backend.invoke(cmd, args);
    } catch (e) {
      if (!opts.silent) showToast(errText(e), "error", "Backend error");
      throw e;
    }
  }

  // ── App state ─────────────────────────────────────────────────────────────
  const state = {
    identity: null,
    workspace: null,
    chitthis: [],
    seenChitthi: new Set(),
    dms: new Map(), // sender npub -> [{ content, created_at, outgoing, upi }]
    activeContact: null,
    coupleRole: "sakha",
  };

  // ── Screen + theme management (progressive disclosure) ────────────────────
  function setScreen(name) {
    document.body.dataset.screen = name;
    $("#screen-vault").hidden = name !== "vault";
    $("#screen-app").hidden = name !== "app";
    $("#screen-couple").hidden = name !== "couple";
  }

  function themeClass(key) {
    switch (key) {
      case "OffGridTravel":
        return "theme-travel";
      case "CoupleSandboxSakha":
        return "theme-couple-sakha";
      case "CoupleSandboxSakhi":
        return "theme-couple-sakhi";
      default:
        return "theme-base";
    }
  }

  function setPill(node, on) {
    node.classList.toggle("on", !!on);
    node.classList.toggle("off", !on);
  }

  /** Apply a WorkspaceDto: re-theme, update indicators, pick the right screen. */
  function applyWorkspace(ws) {
    if (!ws) return;
    state.workspace = ws;
    document.body.className = themeClass(ws.key);

    $("#ws-badge").textContent = ws.mesh_active
      ? "Off-Grid"
      : ws.couple_sandbox
        ? "Couples"
        : "Base";
    setPill($("#pill-relays"), ws.relay_connected);
    setPill($("#pill-mesh"), ws.mesh_active);
    $("#travel-toggle").checked = !!ws.mesh_active;

    if (ws.couple_sandbox) {
      $("#couple-role").textContent = ws.key.endsWith("Sakhi") ? "Sakhi" : "Sakha";
      setScreen("couple");
    } else {
      setScreen("app");
    }
  }

  // ── Milestone 1: Vault initialization ─────────────────────────────────────
  async function handleUnlock(e) {
    e.preventDefault();
    const pass = $("#passphrase").value.trim();
    if (!pass) {
      showToast("Enter a passphrase to continue", "warn");
      return;
    }
    const btn = $("#unlock-btn");
    setBusy(btn, true);
    try {
      const path = await resolveStorePath();
      const id = await safeInvoke("unlock_comrade_vault", {
        path,
        passphrase: pass,
      });
      state.identity = id;
      $("#identity-npub").textContent = shortNpub(id.npub);
      $("#passphrase").value = "";
      showToast(`Vault unlocked · ${shortNpub(id.npub)}`, "success");

      const ws = await safeInvoke("current_workspace", undefined, {
        silent: true,
      }).catch(() => ({
        key: "Base",
        label: "Base",
        relay_connected: true,
        mesh_active: false,
        couple_sandbox: false,
      }));
      applyWorkspace(ws);
      await loadTimeline();
    } catch {
      /* error already toasted */
    } finally {
      setBusy(btn, false);
    }
  }

  // ── Milestone 2: Sabha timeline ───────────────────────────────────────────
  async function loadTimeline() {
    const loading = $("#sabha-loading");
    const empty = $("#sabha-empty");
    const feed = $("#sabha-feed");
    empty.hidden = true;
    feed.hidden = true;
    loading.hidden = false;
    try {
      const items = await safeInvoke("fetch_sabha_timeline");
      state.chitthis = Array.isArray(items) ? items : [];
      state.seenChitthi = new Set(state.chitthis.map((c) => c.id));
      renderFeed();
    } catch {
      /* toasted */
    } finally {
      loading.hidden = true;
    }
  }

  function chitthiCard(c, isNew = false) {
    return el(
      "article",
      { class: "chitthi" + (isNew ? " is-new" : "") },
      el(
        "div",
        { class: "chitthi-meta" },
        el("span", { class: "chitthi-author", text: shortNpub(c.author || "anon") }),
        el("span", { class: "chitthi-time", text: relTime(c.created_at) }),
      ),
      el("div", { class: "chitthi-body", text: c.content || "" }),
      c.reply_to
        ? el("div", {
            class: "chitthi-reply",
            text: `↳ reply to ${String(c.reply_to).slice(0, 12)}…`,
          })
        : null,
    );
  }

  function renderFeed() {
    const feed = $("#sabha-feed");
    const empty = $("#sabha-empty");
    feed.innerHTML = "";
    if (!state.chitthis.length) {
      empty.hidden = false;
      feed.hidden = true;
      return;
    }
    empty.hidden = true;
    feed.hidden = false;
    for (const c of state.chitthis) feed.append(chitthiCard(c));
  }

  /** Milestone 3: seamlessly prepend a freshly received/sent Chitthi. */
  function prependChitthi(c, isNew = false) {
    if (c.id && state.seenChitthi.has(c.id)) return;
    if (c.id) state.seenChitthi.add(c.id);
    state.chitthis.unshift(c);
    $("#sabha-empty").hidden = true;
    const feed = $("#sabha-feed");
    feed.hidden = false;
    feed.prepend(chitthiCard(c, isNew));
  }

  async function handleBroadcast() {
    const input = $("#chitthi-input");
    const content = input.value.trim();
    if (!content) {
      showToast("Write a Chitthi first", "warn");
      return;
    }
    const btn = $("#broadcast-btn");
    setBusy(btn, true);
    try {
      const id = await safeInvoke("broadcast_chitthi", { content, replyTo: null });
      input.value = "";
      updateCount();
      showToast("Chitthi broadcast to Sabha", "success");
      prependChitthi(
        {
          id,
          author: state.identity ? state.identity.npub : "you",
          content,
          created_at: nowSecs(),
          reply_to: null,
        },
        true,
      );
    } catch {
      /* toasted */
    } finally {
      setBusy(btn, false);
    }
  }

  function updateCount() {
    const v = $("#chitthi-input").value;
    $("#chitthi-count").textContent = `${v.length} / 2000`;
  }

  // ── Tabs ──────────────────────────────────────────────────────────────────
  function switchTab(name) {
    for (const t of document.querySelectorAll(".tab")) {
      const on = t.dataset.tab === name;
      t.classList.toggle("is-active", on);
      t.setAttribute("aria-selected", on ? "true" : "false");
    }
    $("#view-sabha").hidden = name !== "sabha";
    $("#view-vault").hidden = name !== "vault";
  }

  // ── Milestone 2/3: Vault DMs ──────────────────────────────────────────────
  function onIncomingDm(p) {
    const key = p.sender || "unknown";
    const list = state.dms.get(key) || [];
    list.push({
      content: p.content || "",
      created_at: p.created_at,
      outgoing: false,
      upi: p.upi_intents || [],
    });
    state.dms.set(key, list);
    renderContacts();
    if (state.activeContact === key) renderConversation();
    showToast(`New encrypted DM from ${shortNpub(key)}`, "info");
  }

  function renderContacts() {
    const list = $("#contact-list");
    const empty = $("#contacts-empty");
    list.innerHTML = "";
    const keys = [...state.dms.keys()];
    empty.hidden = keys.length > 0;
    for (const k of keys) {
      const msgs = state.dms.get(k);
      const last = msgs[msgs.length - 1];
      list.append(
        el(
          "li",
          {
            class: "contact" + (k === state.activeContact ? " is-active" : ""),
            onClick: () => selectContact(k),
          },
          el("span", { class: "contact-name", text: shortNpub(k) }),
          el("span", { class: "contact-last", text: last ? last.content : "" }),
        ),
      );
    }
  }

  function selectContact(key) {
    state.activeContact = key;
    $("#dm-input").disabled = false;
    renderContacts();
    renderConversation();
  }

  function renderConversation() {
    const log = $("#chat-log");
    const head = $("#chat-header");
    log.innerHTML = "";
    head.innerHTML = "";
    if (!state.activeContact) {
      head.append(el("span", { class: "muted", text: "Select a conversation" }));
      return;
    }
    head.textContent = shortNpub(state.activeContact);
    const msgs = state.dms.get(state.activeContact) || [];
    for (const m of msgs) {
      log.append(
        el(
          "div",
          { class: "bubble " + (m.outgoing ? "out" : "in") },
          el("span", { text: m.content }),
          el("span", { class: "bubble-time", text: relTime(m.created_at) }),
        ),
      );
      if (m.upi && m.upi.length) {
        for (const i of m.upi)
          log.append(
            el("div", {
              class: "upi-chip",
              text: `₹${Number(i.amount_inr).toFixed(2)} → ${i.vpa}`,
            }),
          );
      }
    }
    log.scrollTop = log.scrollHeight;
  }

  // Live UPI /pay detection in the DM composer (real extract_payments command).
  const handleDmInput = debounce(async () => {
    const text = $("#dm-input").value;
    const preview = $("#dm-upi-preview");
    if (!text.includes("/pay")) {
      preview.hidden = true;
      preview.innerHTML = "";
      return;
    }
    try {
      const intents = await safeInvoke("extract_payments", { text }, { silent: true });
      preview.innerHTML = "";
      if (intents && intents.length) {
        preview.hidden = false;
        for (const i of intents)
          preview.append(
            el("div", {
              class: "upi-chip",
              text: `Detected: ₹${Number(i.amount_inr).toFixed(2)} → ${i.vpa}`,
            }),
          );
      } else {
        preview.hidden = true;
      }
    } catch {
      preview.hidden = true;
    }
  }, 250);

  // ── Milestone 4: Travel / Off-Grid toggle ─────────────────────────────────
  async function handleTravel(e) {
    const want = e.target.checked;
    const target = want ? "OffGridTravel" : "Base";
    try {
      const ws = await safeInvoke("toggle_app_workspace", { target });
      applyWorkspace(ws);
      showToast(
        want
          ? "Off-Grid / Travel mode — public relays paused, Saathi mesh active"
          : "Back on the public relays",
        "info",
      );
    } catch {
      e.target.checked = !want; // revert the switch on a blocked transition
    }
  }

  // ── Milestone 4: Partner Portal (Couple Sandbox) ──────────────────────────
  function openPartnerModal() {
    $("#modal-partner").hidden = false;
    $("#pair-payload").focus();
  }
  function closePartnerModal() {
    $("#modal-partner").hidden = true;
  }

  async function handlePair() {
    const payload = $("#pair-payload").value.trim();
    const role = (document.querySelector("input[name=pair-role]:checked") || {}).value || "sakha";
    // Client-side validation of the cryptographic pairing payload.
    if (payload.length < 8) {
      showToast("Enter a valid pairing payload", "warn");
      return;
    }
    if (!/^npub1[0-9a-z]+$/i.test(payload) && !payload.includes(":")) {
      showToast("That doesn't look like a valid pairing token", "warn");
      return;
    }
    const btn = $("#pair-submit");
    setBusy(btn, true);
    try {
      const target = role === "sakhi" ? "CoupleSandboxSakhi" : "CoupleSandboxSakha";
      const ws = await safeInvoke("toggle_app_workspace", { target });
      state.coupleRole = role;
      closePartnerModal();
      applyWorkspace(ws);
      showToast("Partner portal unlocked", "success");
    } catch {
      /* e.g. blocked because Travel mode is active — toasted already */
    } finally {
      setBusy(btn, false);
    }
  }

  async function exitCouple() {
    try {
      const ws = await safeInvoke("toggle_app_workspace", { target: "Base" });
      applyWorkspace(ws);
    } catch {
      /* toasted */
    }
  }

  async function handleSyncLedger() {
    const btn = $("#sync-ledger-btn");
    const status = $("#ledger-status");
    setBusy(btn, true);
    status.textContent = "Syncing the shared ledger…";
    try {
      const id = await safeInvoke("sync_ledger");
      status.textContent = `Synced ✓  ·  event ${String(id).slice(0, 16)}…`;
      showToast("Hisab-Kitab ledger synced", "success");
    } catch (e) {
      status.textContent = `Sync unavailable — ${errText(e)}`;
    } finally {
      setBusy(btn, false);
    }
  }

  // ── Milestone 3: real-time event wiring ───────────────────────────────────
  async function wireEvents() {
    try {
      await backend.listen(EVENT_CHANNEL, (evt) => {
        const p = evt && evt.payload;
        if (!p || !p.type) return;
        if (p.type === "incoming_chitthi") {
          prependChitthi(
            {
              id: p.id,
              author: p.author,
              content: p.content,
              created_at: p.created_at,
              reply_to: p.reply_to,
            },
            true,
          );
        } else if (p.type === "incoming_direct_message") {
          onIncomingDm(p);
        }
      });
    } catch (e) {
      showToast(`Could not subscribe to live events: ${errText(e)}`, "warn");
    }
  }

  // ── Wiring ────────────────────────────────────────────────────────────────
  function init() {
    if (!hasTauri) $("#preview-banner").hidden = false;

    $("#vault-form").addEventListener("submit", handleUnlock);
    $("#toggle-reveal").addEventListener("click", () => {
      const i = $("#passphrase");
      i.type = i.type === "password" ? "text" : "password";
    });

    $("#identity-chip").addEventListener("click", async () => {
      if (!state.identity) return;
      try {
        await navigator.clipboard.writeText(state.identity.npub);
        showToast("npub copied to clipboard", "success");
      } catch {
        showToast("Clipboard unavailable", "error");
      }
    });

    for (const t of document.querySelectorAll(".tab"))
      t.addEventListener("click", () => switchTab(t.dataset.tab));

    $("#chitthi-input").addEventListener("input", updateCount);
    $("#broadcast-btn").addEventListener("click", handleBroadcast);
    $("#dm-input").addEventListener("input", handleDmInput);
    $("#dm-send").addEventListener("click", () =>
      showToast("Outbound DM transmission needs a backend send_dm command.", "info"),
    );

    $("#travel-toggle").addEventListener("change", handleTravel);
    $("#partner-btn").addEventListener("click", openPartnerModal);
    $("#partner-cancel").addEventListener("click", closePartnerModal);
    $("#pair-submit").addEventListener("click", handlePair);
    $("#couple-exit").addEventListener("click", exitCouple);
    $("#sync-ledger-btn").addEventListener("click", handleSyncLedger);

    $("#modal-partner").addEventListener("click", (e) => {
      if (e.target === $("#modal-partner")) closePartnerModal();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !$("#modal-partner").hidden) closePartnerModal();
    });

    wireEvents();
    renderContacts();
    renderConversation();
    setScreen("vault");
    $("#passphrase").focus();
  }

  // ── Dev mock backend (used only when running outside the Tauri shell) ──────
  function mockBackend() {
    const listeners = {};
    const wsOf = (key) => ({
      key,
      label: key,
      active: true,
      relay_connected: key !== "OffGridTravel",
      mesh_active: key === "OffGridTravel",
      couple_sandbox: key.startsWith("CoupleSandbox"),
    });
    let ws = wsOf("Base");
    const delay = (ms) => new Promise((r) => setTimeout(r, ms));
    const re = /\/pay\s+(\d+(?:\.\d{1,2})?)\s+to\s+([a-zA-Z0-9.\-_]+@[a-zA-Z0-9]+)/gi;

    const invoke = async (cmd, args = {}) => {
      await delay(120);
      switch (cmd) {
        case "unlock_comrade_vault":
          return { npub: "npub1mockdev0identity00000000000000000000000000000000", has_secret: true };
        case "current_identity":
          return { npub: "npub1mockdev0identity00000000000000000000000000000000", has_secret: true };
        case "current_workspace":
          return ws;
        case "toggle_app_workspace":
        case "switch_workspace":
          ws = wsOf(args.target || args.key || "Base");
          return ws;
        case "back":
          ws = wsOf("Base");
          return ws;
        case "fetch_sabha_timeline":
          return [
            { id: "demo1", author: "npub1alice000000000000000000000000000000000000000000", content: "Namaste from the Sabha! (mock)", created_at: nowSecs() - 600, reply_to: null },
            { id: "demo2", author: "npub1bob0000000000000000000000000000000000000000000000", content: "Off-grid travel mode is wild.", created_at: nowSecs() - 90, reply_to: "demo1" },
          ];
        case "broadcast_chitthi":
          return "mock_" + Date.now();
        case "extract_payments": {
          const out = [];
          let m;
          re.lastIndex = 0;
          while ((m = re.exec(args.text || "")) !== null)
            out.push({ amount_inr: parseFloat(m[1]), vpa: m[2], uri: `upi://pay?pa=${m[2]}&am=${m[1]}` });
          return out;
        }
        case "sync_ledger":
          throw "no shared secret available — pairing handshake incomplete";
        default:
          throw `mock backend: unknown command '${cmd}'`;
      }
    };

    const listen = async (event, cb) => {
      (listeners[event] = listeners[event] || []).push(cb);
      return () => {};
    };
    // Manual event injection for design/QA: window.__comradeEmit({type:'incoming_chitthi', ...})
    window.__comradeEmit = (payload) =>
      (listeners[EVENT_CHANNEL] || []).forEach((cb) => cb({ payload }));

    return { invoke, listen };
  }

  if (document.readyState === "loading")
    document.addEventListener("DOMContentLoaded", init);
  else init();
})();
