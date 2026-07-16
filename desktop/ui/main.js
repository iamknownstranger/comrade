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
  const MAX_MEDIA_BYTES = 10 * 1024 * 1024; // 10 MB hard limit (Milestone 5)

  // ── Backend access (real Tauri, or a dev mock for browser preview) ────────
  const TAURI = window.__TAURI__;
  const hasTauri = !!(TAURI && TAURI.core && TAURI.event);
  const backend = hasTauri
    ? {
        invoke: (cmd, args) => TAURI.core.invoke(cmd, args),
        listen: (event, cb) => TAURI.event.listen(event, cb),
      }
    : mockBackend();

  // ── Call decision helpers (desktop/ui/call_decisions.mjs) ──────────────────
  // This file is a plain classic script (index.html loads it as
  // `<script src="main.js">`, no type="module"), so a static `import`
  // statement isn't available here. Dynamic `import()` is spec'd to work
  // from classic scripts too (it isn't restricted to module scripts), so we
  // kick the load off once, at parse time — long before any call signal can
  // plausibly arrive — and every call site below awaits this same cached
  // promise. That's a smaller, safer diff than flipping main.js to
  // type="module" (which would also change the whole file to always-strict
  // semantics and defer-by-default execution timing) for a change that only
  // needs to reuse a couple of pure functions from another file.
  const callDecisionsReady = import("./call_decisions.mjs");

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
    // peer pubkey -> [{ id?, content?, media?, created_at, outgoing, upi, status?, reply_to? }]
    dms: new Map(),
    activeContact: null,
    coupleRole: "sakha",
    partnerNpub: null,
    // Milestone 6: comms
    requests: [], // pending stranger DMs: [{ peer, last_message, last_at }]
    peerNames: new Map(), // peer pubkey -> published display handle
    replyTo: null, // { id, content, outgoing } while composing a reply
    call: null, // active call session (see newCallState)
    // Bounded memory of recently-ended call ids (see call_decisions.mjs
    // rememberEndedCall) — mirrors Android's CallManager.endedCallIds, so a
    // redelivered Offer for a call we already tore down doesn't ring again.
    endedCallIds: [],
  };

  // Prefer a peer's published handle over the raw npub when we have one.
  function displayName(peer) {
    const n = state.peerNames.get(peer);
    return n ? n : shortNpub(peer);
  }

  // ── Media helpers ─────────────────────────────────────────────────────────
  function fileToBase64(file) {
    return new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => {
        const s = String(r.result);
        resolve(s.slice(s.indexOf(",") + 1)); // strip "data:...;base64,"
      };
      r.onerror = () => reject(r.error);
      r.readAsDataURL(file);
    });
  }

  function base64ToBlob(b64, mime) {
    const bin = atob(b64);
    const arr = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
    return new Blob([arr], { type: mime || "application/octet-stream" });
  }

  function renderMediaEl(mime, url) {
    if (mime.startsWith("image/")) return el("img", { class: "media-img", src: url, alt: "media" });
    if (mime.startsWith("audio/")) return el("audio", { controls: "", src: url });
    if (mime.startsWith("video/")) return el("video", { class: "media-img", controls: "", src: url });
    return el("a", { href: url, download: "comrade-media", text: "Download file" });
  }

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
      // Pull the authoritative pairing state (partner key, ledger form
      // enablement, ledger content) — fire-and-forget so entering the
      // screen never blocks on it.
      refreshSakhaStatus().catch(() => {});
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
      await loadConversations();
      await loadRequests();
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
      id: p.id,
      content: p.content || "",
      created_at: p.created_at,
      outgoing: false,
      upi: p.upi_intents || [],
      reply_to: p.reply_to || null,
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
          el("span", { class: "contact-name", text: displayName(k) }),
          el("span", {
            class: "contact-last",
            text: last
              ? last.content ||
                (last.media ? `📎 ${last.media.caption || "media"}` : "")
              : "",
          }),
        ),
      );
    }
  }

  /** Seed the contact list from the persisted offline history (chat list). */
  async function loadConversations() {
    let convos;
    try {
      convos = await safeInvoke("conversations", undefined, { silent: true });
    } catch {
      return; // older backend without the command — live events still work
    }
    for (const c of convos || []) {
      if (!state.dms.has(c.peer)) {
        state.dms.set(c.peer, [
          { content: c.last_message, created_at: c.last_at, outgoing: !!c.last_outgoing, upi: [] },
        ]);
      }
    }
    renderContacts();
  }

  function selectContact(key) {
    state.activeContact = key;
    clearReply();
    $("#dm-input").disabled = false;
    $("#dm-attach").disabled = false;
    $("#dm-send").disabled = false;
    renderContacts();
    renderConversation();
    // Opening a conversation clears its unread state and sends read receipts.
    safeInvoke("mark_conversation_read", { peer: key }, { silent: true }).catch(() => {});
    // Pull the full persisted thread — text history plus persisted media
    // history — and merge in any live media bubbles from this session ahead
    // of their persisted duplicate (a live one may already hold a decrypted
    // objectUrl, which a freshly-fetched persisted row never does).
    Promise.all([
      safeInvoke("messages_with", { peer: key }, { silent: true }).catch(() => []),
      safeInvoke("media_with", { peer: key }, { silent: true }).catch(() => []),
    ]).then(([msgs, mediaHistory]) => {
      if (state.activeContact !== key) return;
      const texts = Array.isArray(msgs) ? msgs : [];
      const liveMedia = (state.dms.get(key) || []).filter((m) => m.media);
      const seenEventIds = new Set(liveMedia.map((m) => m.media.eventId));
      const persistedMedia = (Array.isArray(mediaHistory) ? mediaHistory : [])
        .filter((m) => !seenEventIds.has(m.event_id))
        .map((m) => ({
          created_at: m.created_at,
          outgoing: !!m.outgoing,
          media: { eventId: m.event_id, mime: m.mime_type, caption: m.caption },
        }));
      if (!texts.length && !liveMedia.length && !persistedMedia.length) return;
      const merged = texts
        .map((m) => ({
          id: m.id,
          content: m.content,
          created_at: m.created_at,
          outgoing: !!m.outgoing,
          upi: [],
          status: m.status || null,
          reply_to: m.reply_to || null,
        }))
        .concat(liveMedia)
        .concat(persistedMedia)
        .sort((a, b) => a.created_at - b.created_at);
      state.dms.set(key, merged);
      renderConversation();
    });
  }

  /** Send the composed DM to the active contact (real end-to-end send). */
  async function handleDmSend() {
    const input = $("#dm-input");
    const content = input.value.trim();
    if (!content) return;
    if (!state.activeContact) {
      showToast("Select a conversation first", "warn");
      return;
    }
    const btn = $("#dm-send");
    const replyTo = state.replyTo;
    setBusy(btn, true);
    try {
      const msg = replyTo
        ? await safeInvoke("send_dm_reply", {
            target: state.activeContact,
            content,
            replyTo: replyTo.id,
          })
        : await safeInvoke("send_dm", {
            target: state.activeContact,
            content,
          });
      input.value = "";
      const preview = $("#dm-upi-preview");
      preview.hidden = true;
      preview.innerHTML = "";
      clearReply();
      const list = state.dms.get(state.activeContact) || [];
      list.push({
        id: msg.id,
        content: msg.content,
        created_at: msg.created_at,
        outgoing: true,
        upi: [],
        status: msg.status || "sent",
        reply_to: msg.reply_to || (replyTo ? replyTo.id : null),
      });
      state.dms.set(state.activeContact, list);
      renderContacts();
      renderConversation();
    } catch {
      /* toasted */
    } finally {
      setBusy(btn, false);
    }
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
    const peer = state.activeContact;
    head.append(
      el("span", { class: "chat-peer mono", text: displayName(peer) }),
      el(
        "div",
        { class: "chat-actions" },
        el("button", {
          class: "icon-btn",
          title: "Voice call",
          "aria-label": "Start voice call",
          text: "📞",
          onClick: () => startCall(peer, "audio"),
        }),
        el("button", {
          class: "icon-btn",
          title: "Video call",
          "aria-label": "Start video call",
          text: "🎥",
          onClick: () => startCall(peer, "video"),
        }),
      ),
    );
    const msgs = state.dms.get(state.activeContact) || [];
    for (const m of msgs) {
      log.append(m.media ? mediaBubble(m) : textBubble(m));
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

  function textBubble(m) {
    const wrap = el("div", { class: "bubble " + (m.outgoing ? "out" : "in") });
    if (m.reply_to) wrap.append(quotePreview(m.reply_to));
    wrap.append(el("span", { class: "bubble-text", text: m.content }));
    wrap.append(
      el(
        "div",
        { class: "bubble-meta" },
        el("span", { class: "bubble-time", text: relTime(m.created_at) }),
        m.outgoing && m.status ? statusTick(m.status) : null,
      ),
    );
    // A reply is only addressable if we know the target message's event id.
    if (m.id) wrap.append(replyButton(m));
    return wrap;
  }

  // ── Milestone 6: replies, receipts, requests, calls ───────────────────────

  /** A quoted preview of the replied-to message, looked up in the open thread. */
  function quotePreview(replyToId) {
    const msgs = state.dms.get(state.activeContact) || [];
    const q = msgs.find((x) => x.id && x.id === replyToId);
    const text = q
      ? q.content || (q.media ? `📎 ${q.media.caption || "media"}` : "message")
      : "Original message";
    return el(
      "div",
      { class: "bubble-quote" },
      el("span", { class: "bubble-quote-text", text: text }),
    );
  }

  /** Delivery-status ticks for an outgoing bubble. */
  function statusTick(status) {
    const glyph = status === "sent" ? "✓" : "✓✓";
    return el("span", {
      class: "bubble-status" + (status === "read" ? " read" : ""),
      title: status,
      text: glyph,
    });
  }

  function replyButton(m) {
    return el("button", {
      class: "bubble-reply",
      title: "Reply",
      "aria-label": "Reply to this message",
      text: "↩",
      onClick: (e) => {
        e.stopPropagation();
        setReply(m);
      },
    });
  }

  function setReply(m) {
    if (!m || !m.id) return;
    const content = m.content || (m.media ? `📎 ${m.media.caption || "media"}` : "message");
    state.replyTo = { id: m.id, content, outgoing: !!m.outgoing };
    $("#dm-reply-text").textContent = content;
    $("#dm-reply-chip").hidden = false;
    const input = $("#dm-input");
    if (!input.disabled) input.focus();
  }

  function clearReply() {
    state.replyTo = null;
    const chip = $("#dm-reply-chip");
    if (chip) chip.hidden = true;
  }

  // ── Delivered / read receipts ──────────────────────────────────────────────
  const STATUS_RANK = { sent: 1, delivered: 2, read: 3 };

  function onMessageStatus(p) {
    const list = state.dms.get(p.peer);
    if (!list) return;
    const ids = new Set(p.message_ids || []);
    const next = p.status;
    let changed = false;
    for (const m of list) {
      if (!m.outgoing || !m.id || !ids.has(m.id)) continue;
      // Never regress a status (a late "delivered" must not undo "read").
      if ((STATUS_RANK[next] || 0) >= (STATUS_RANK[m.status] || 0)) {
        m.status = next;
        changed = true;
      }
    }
    if (changed && state.activeContact === p.peer) renderConversation();
  }

  function onPeerProfileUpdated(p) {
    if (!p.peer) return;
    if (p.name) state.peerNames.set(p.peer, p.name);
    else state.peerNames.delete(p.peer);
    renderContacts();
    renderRequests();
    if (state.activeContact === p.peer) renderConversation();
  }

  // ── Message requests (stranger DMs awaiting accept/block) ──────────────────
  async function loadRequests() {
    let reqs;
    try {
      reqs = await safeInvoke("message_requests", undefined, { silent: true });
    } catch {
      return; // older backend without the command
    }
    state.requests = Array.isArray(reqs) ? reqs : [];
    renderRequests();
  }

  function renderRequests() {
    const section = $("#requests-section");
    if (!section) return;
    const list = $("#requests-list");
    const count = $("#requests-count");
    list.innerHTML = "";
    const reqs = state.requests || [];
    section.hidden = reqs.length === 0;
    count.textContent = reqs.length ? String(reqs.length) : "";
    for (const r of reqs) {
      list.append(
        el(
          "li",
          { class: "request" },
          el(
            "div",
            { class: "request-info" },
            el("span", { class: "request-name mono", text: displayName(r.peer) }),
            el("span", { class: "request-last", text: r.last_message || "" }),
          ),
          el(
            "div",
            { class: "request-actions" },
            el("button", {
              class: "btn btn-primary btn-sm",
              text: "Accept",
              onClick: () => acceptRequest(r.peer),
            }),
            el("button", {
              class: "btn btn-ghost btn-sm",
              text: "Block",
              onClick: () => blockRequest(r.peer),
            }),
          ),
        ),
      );
    }
  }

  async function acceptRequest(peer) {
    try {
      await safeInvoke("accept_request", { peer });
    } catch {
      return; // toasted
    }
    state.requests = (state.requests || []).filter((r) => r.peer !== peer);
    renderRequests();
    showToast(`Request from ${shortNpub(peer)} accepted`, "success");
    loadRequests().catch(() => {});
    loadConversations().catch(() => {});
  }

  async function blockRequest(peer) {
    try {
      await safeInvoke("block_conversation", { peer });
    } catch {
      return; // toasted
    }
    state.requests = (state.requests || []).filter((r) => r.peer !== peer);
    renderRequests();
    showToast(`${shortNpub(peer)} blocked`, "info");
    loadRequests().catch(() => {});
    loadConversations().catch(() => {});
  }

  function onIncomingMessageRequest(p) {
    const rec = { peer: p.peer, last_message: p.last_message, last_at: p.last_at };
    const i = (state.requests || []).findIndex((r) => r.peer === p.peer);
    if (i >= 0) state.requests[i] = rec;
    else state.requests.unshift(rec);
    renderRequests();
    showToast(`New message request from ${shortNpub(p.peer)}`, "info");
  }

  // ── TURN relay (call settings) ─────────────────────────────────────────────
  function openTurnModal() {
    $("#modal-turn").hidden = false;
    $("#turn-url").focus();
  }
  function closeTurnModal() {
    $("#modal-turn").hidden = true;
  }
  async function handleSaveTurn() {
    const url = $("#turn-url").value.trim();
    const username = $("#turn-username").value.trim();
    const credential = $("#turn-credential").value.trim();
    const btn = $("#turn-save");
    setBusy(btn, true);
    try {
      await safeInvoke("set_turn_server", { url, username, credential });
      showToast(url ? "TURN relay saved" : "TURN relay cleared", "success");
      closeTurnModal();
    } catch {
      /* toasted */
    } finally {
      setBusy(btn, false);
    }
  }

  // ── WebRTC 1:1 voice / video calls ─────────────────────────────────────────
  //
  // Signaling rides the E2E DM channel: we hand a CallSignal JSON string to
  // `send_call_signal`, and receive the peer's signals as `incoming_call_signal`
  // events. WebRTC itself (getUserMedia + RTCPeerConnection) runs in the webview.
  // One call at a time; `state.call` holds the whole session.

  function callSupported() {
    return !!(
      navigator.mediaDevices &&
      navigator.mediaDevices.getUserMedia &&
      window.RTCPeerConnection
    );
  }

  function newCallState(base) {
    return Object.assign(
      {
        callId: null,
        peer: null,
        media: "audio",
        incoming: false,
        phase: "calling", // calling | ringing | connecting | connected
        pc: null,
        localStream: null,
        remoteStream: null,
        offerSdp: null, // buffered offer (callee) until Accept
        pendingIce: [], // remote candidates buffered until remoteDescription set
        remoteSet: false,
        connected: false,
        startedAt: null, // connect time (unix secs) that drives the timer
        initAt: nowSecs(), // call-initiation time; the log's started_at fallback
        timerId: null,
        muted: false,
        ended: false,
      },
      base,
    );
  }

  // Map ice-server DTOs -> RTCIceServer, dropping null auth fields.
  function normalizeIce(list) {
    return (list || [])
      .map((s) => {
        const o = { urls: s.urls };
        if (s.username != null) o.username = s.username;
        if (s.credential != null) o.credential = s.credential;
        return o;
      })
      .filter((o) => o.urls && o.urls.length);
  }

  async function sendSignal(sig) {
    const c = state.call;
    if (!c) return;
    try {
      await safeInvoke(
        "send_call_signal",
        {
          peer: c.peer,
          callId: c.callId,
          media: c.media,
          signalJson: JSON.stringify(sig),
        },
        { silent: true },
      );
    } catch {
      /* ICE loss is tolerable; a dropped offer/answer fails the call cleanly */
    }
  }

  // Shared peer-connection setup for both the caller and the accepting callee.
  async function setupPeer(iceServers) {
    const c = state.call;
    let stream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({
        audio: true,
        video: c.media === "video",
      });
    } catch (e) {
      showToast(`Microphone/camera unavailable — ${errText(e)}`, "error");
      await finishCall({
        sendHangup: true,
        reason: c.incoming ? "declined" : "failed",
        outcome: "failed",
      });
      return false;
    }
    let pc;
    try {
      pc = new RTCPeerConnection({ iceServers: normalizeIce(iceServers) });
    } catch (e) {
      showToast(`Could not start WebRTC — ${errText(e)}`, "error");
      stream.getTracks().forEach((t) => t.stop());
      await finishCall({ sendHangup: true, reason: "failed", outcome: "failed" });
      return false;
    }
    c.localStream = stream;
    c.pc = pc;
    c.remoteStream = new MediaStream();
    for (const track of stream.getTracks()) pc.addTrack(track, stream);

    pc.onicecandidate = (ev) => {
      if (!ev.candidate) return; // null == end-of-candidates
      sendSignal({
        kind: "ice",
        candidate: ev.candidate.candidate,
        sdp_mid: ev.candidate.sdpMid == null ? undefined : ev.candidate.sdpMid,
        sdp_m_line_index:
          ev.candidate.sdpMLineIndex == null ? undefined : ev.candidate.sdpMLineIndex,
      });
    };
    pc.ontrack = (ev) => {
      if (ev.streams && ev.streams[0]) c.remoteStream = ev.streams[0];
      else c.remoteStream.addTrack(ev.track);
      attachRemoteMedia();
    };
    pc.onconnectionstatechange = () => {
      const st = pc.connectionState;
      if (st === "connected") onCallConnected();
      else if (st === "failed")
        finishCall({ sendHangup: true, reason: "failed", outcome: "failed" });
      // "disconnected" can be transient (ICE restart); don't tear down on it.
    };

    attachLocalMedia();
    attachRemoteMedia();
    return true;
  }

  // Caller: place the call, negotiate locally, and send the offer.
  async function startCall(peer, media) {
    if (!peer) {
      showToast("Select a conversation first", "warn");
      return;
    }
    if (state.call) {
      showToast("You're already in a call", "warn");
      return;
    }
    if (!callSupported()) {
      showToast("Calling isn't available in this environment", "error");
      return;
    }
    let session;
    try {
      session = await safeInvoke("place_call", { peer, media });
    } catch {
      return; // toasted
    }
    state.call = newCallState({
      callId: session.call_id,
      peer: session.peer || peer,
      media: session.media || media,
      incoming: false,
      phase: "calling",
    });
    showCallOverlay();
    setCallStatusText("Calling…");
    const ok = await setupPeer(session.ice_servers || []);
    if (!ok) return; // setupPeer handled cleanup
    try {
      const offer = await state.call.pc.createOffer();
      await state.call.pc.setLocalDescription(offer);
      await sendSignal({ kind: "offer", sdp: offer.sdp });
    } catch (e) {
      showToast(`Could not start the call — ${errText(e)}`, "error");
      await finishCall({ sendHangup: true, reason: "failed", outcome: "failed" });
    }
  }

  // Callee: an offer arrived. Depending on call_decisions.decideOfferDisposition
  // this either rings fresh (no call yet), renegotiates the existing pc (a
  // same-call_id re-offer — e.g. the caller's STUN->TURN ICE-restart
  // fallback), silently no-ops a duplicate/ended call_id, or auto-rejects as
  // busy (a genuinely different call_id). See call_decisions.mjs for the
  // pure decision this mirrors from Android's CallManager.
  async function handleIncomingOffer(p, sig) {
    const { decideOfferDisposition, OFFER_DISPOSITION, isEndedCallId } = await callDecisionsReady;
    const c = state.call;
    const disposition = decideOfferDisposition({
      hasCall: !!c,
      sameCallId: !!c && c.callId === p.call_id,
      hasPc: !!c && !!c.pc,
      isEndedCallId: isEndedCallId(state.endedCallIds, p.call_id),
    });

    if (disposition === OFFER_DISPOSITION.ENDED_NOOP) {
      // Redelivered offer for a call we already tore down (relay
      // at-least-once delivery, or a backfill re-scan) — drop silently,
      // don't ring again.
      console.log(`call ${p.call_id}: ignoring offer for an already-ended call`);
      return;
    }

    if (disposition === OFFER_DISPOSITION.RENEGOTIATE) {
      // Same call_id re-offer on a live pc (the P0 fix: this used to be
      // answered `busy`, which broke an Android caller's STUN->TURN
      // ICE-restart fallback). Answer on the existing pc — do NOT touch
      // getUserMedia, do NOT recreate the pc, do NOT reset the duration
      // timer or any UI state.
      try {
        await c.pc.setRemoteDescription({ type: "offer", sdp: sig.sdp });
        const answer = await c.pc.createAnswer();
        await c.pc.setLocalDescription(answer);
        await sendSignal({ kind: "answer", sdp: answer.sdp });
      } catch (e) {
        showToast(`Could not renegotiate the call — ${errText(e)}`, "error");
      }
      return;
    }

    if (disposition === OFFER_DISPOSITION.DUPLICATE_NOOP) {
      // Same call_id redelivered while still ringing, pre-accept (no pc
      // yet) — drop silently; re-ringing would only restart the ring state.
      return;
    }

    if (disposition === OFFER_DISPOSITION.BUSY) {
      // Genuinely busy on a different call: politely reject the new caller
      // and log the missed attempt.
      try {
        await safeInvoke(
          "send_call_signal",
          {
            peer: p.peer,
            callId: p.call_id,
            media: p.media,
            signalJson: JSON.stringify({ kind: "busy" }),
          },
          { silent: true },
        );
      } catch {
        /* best-effort */
      }
      logCall(p.peer, p.call_id, p.media, true, "busy", nowSecs(), 0);
      return;
    }

    // NEW_INCOMING: no live call — ring as usual (happy path, unchanged).
    if (!callSupported()) {
      try {
        await safeInvoke(
          "hangup_call",
          { peer: p.peer, callId: p.call_id, media: p.media, reason: "failed" },
          { silent: true },
        );
      } catch {
        /* best-effort */
      }
      return;
    }
    state.call = newCallState({
      callId: p.call_id,
      peer: p.peer,
      media: p.media,
      incoming: true,
      phase: "ringing",
      offerSdp: sig.sdp,
    });
    sendSignal({ kind: "ringing" }); // best-effort, not awaited
    showRingingOverlay();
    showToast(
      `Incoming ${p.media === "video" ? "video" : "voice"} call from ${shortNpub(p.peer)}`,
      "info",
    );
  }

  async function acceptIncoming() {
    const c = state.call;
    if (!c || !c.incoming || c.pc) return; // only valid from the ringing phase
    hideRingingOverlay();
    c.phase = "connecting";
    showCallOverlay();
    setCallStatusText("Connecting…");
    let ice = [];
    try {
      ice = (await safeInvoke("call_ice_servers", undefined, { silent: true })) || [];
    } catch {
      /* fall back to host-only candidates */
    }
    const ok = await setupPeer(ice);
    if (!ok) return;
    try {
      await c.pc.setRemoteDescription({ type: "offer", sdp: c.offerSdp });
      c.remoteSet = true;
      await flushPendingIce();
      const answer = await c.pc.createAnswer();
      await c.pc.setLocalDescription(answer);
      await sendSignal({ kind: "answer", sdp: answer.sdp });
    } catch (e) {
      showToast(`Could not answer the call — ${errText(e)}`, "error");
      await finishCall({ sendHangup: true, reason: "failed", outcome: "failed" });
    }
  }

  function declineIncoming() {
    if (!state.call) return;
    finishCall({ sendHangup: true, reason: "declined", outcome: "declined" });
  }

  function hangupByUser() {
    const c = state.call;
    if (!c) return;
    const wasConnected = c.connected;
    finishCall({
      sendHangup: true,
      reason: wasConnected ? "normal" : c.incoming ? "declined" : "cancelled",
      outcome: wasConnected ? "connected" : c.incoming ? "declined" : "cancelled",
    });
  }

  // Route a non-offer signal (answer/ice/ringing/busy/hangup) to the live call.
  function onCallSignal(p) {
    const sig = p.signal || {};
    const kind = sig.kind;
    if (kind === "offer") {
      handleIncomingOffer(p, sig);
      return;
    }
    const c = state.call;
    if (!c || c.callId !== p.call_id) return; // stray, or for a call we've ended
    if (kind === "answer") {
      applyRemoteAnswer(sig.sdp);
    } else if (kind === "ice") {
      addRemoteIce(sig);
    } else if (kind === "ringing") {
      if (!c.connected) setCallStatusText("Ringing…");
    } else if (kind === "busy") {
      showToast(`${displayName(c.peer)} is busy`, "warn");
      finishCall({ sendHangup: false, reason: "busy", outcome: "busy" });
    } else if (kind === "hangup") {
      const reason = sig.reason || "normal";
      const outcome = remoteHangupOutcome(c, reason);
      showToast(
        outcome === "declined" ? `${displayName(c.peer)} declined the call` : "Call ended",
        "info",
      );
      finishCall({ sendHangup: false, reason, outcome });
    }
  }

  async function applyRemoteAnswer(sdp) {
    const c = state.call;
    if (!c || !c.pc) return;
    try {
      await c.pc.setRemoteDescription({ type: "answer", sdp });
      c.remoteSet = true;
      await flushPendingIce();
      if (!c.connected) setCallStatusText("Connecting…");
    } catch (e) {
      await finishCall({ sendHangup: true, reason: "failed", outcome: "failed" });
    }
  }

  async function addRemoteIce(sig) {
    const c = state.call;
    if (!c) return;
    const cand = {
      candidate: sig.candidate,
      sdpMid: sig.sdp_mid == null ? null : sig.sdp_mid,
      sdpMLineIndex: sig.sdp_m_line_index == null ? null : sig.sdp_m_line_index,
    };
    // Buffer until the remote description exists (also covers the ring phase).
    if (!c.pc || !c.remoteSet) {
      c.pendingIce.push(cand);
      return;
    }
    try {
      await c.pc.addIceCandidate(cand);
    } catch {
      /* a rejected candidate shouldn't kill the call */
    }
  }

  async function flushPendingIce() {
    const c = state.call;
    if (!c || !c.pc) return;
    const queued = c.pendingIce.splice(0);
    for (const cand of queued) {
      try {
        await c.pc.addIceCandidate(cand);
      } catch {
        /* ignore */
      }
    }
  }

  function remoteHangupOutcome(c, reason) {
    if (c.connected) return "connected";
    if (c.incoming) return "missed"; // caller cancelled before we answered
    if (reason === "declined") return "declined";
    if (reason === "busy") return "busy";
    if (reason === "missed") return "missed";
    return "cancelled";
  }

  function onCallConnected() {
    const c = state.call;
    if (!c || c.connected) return;
    c.connected = true;
    c.phase = "connected";
    c.startedAt = nowSecs();
    startDurationTimer();
  }

  // Best-effort call-log write (never surfaces its own error).
  function logCall(peer, callId, media, incoming, outcome, startedAt, durationSecs) {
    safeInvoke(
      "log_call",
      { peer, callId, media, incoming, outcome, startedAt, durationSecs },
      { silent: true },
    ).catch(() => {});
  }

  // The single call terminator: optionally signal hangup, log, stop media, hide.
  async function finishCall({ sendHangup, reason, outcome }) {
    const c = state.call;
    if (!c || c.ended) return;
    c.ended = true;
    // Remember this call_id as ended so a redelivered terminal Offer
    // (relay at-least-once delivery, or a backfill re-scan) doesn't ring
    // again — see call_decisions.mjs rememberEndedCall, mirroring Android's
    // CallManager.endedCallIds/rememberEnded.
    const { rememberEndedCall } = await callDecisionsReady;
    state.endedCallIds = rememberEndedCall(state.endedCallIds, c.callId);
    stopDurationTimer();
    const duration = c.startedAt ? Math.max(0, nowSecs() - c.startedAt) : 0;
    if (sendHangup) {
      try {
        await safeInvoke(
          "hangup_call",
          { peer: c.peer, callId: c.callId, media: c.media, reason },
          { silent: true },
        );
      } catch {
        /* best-effort */
      }
    }
    logCall(
      c.peer,
      c.callId,
      c.media,
      c.incoming,
      outcome,
      c.startedAt || c.initAt || nowSecs(),
      duration,
    );
    teardownMedia(c);
    hideCallOverlay();
    hideRingingOverlay();
    state.call = null;
  }

  function teardownMedia(c) {
    try {
      if (c.pc) {
        c.pc.onicecandidate = null;
        c.pc.ontrack = null;
        c.pc.onconnectionstatechange = null;
        c.pc.close();
      }
    } catch {
      /* ignore */
    }
    try {
      if (c.localStream) c.localStream.getTracks().forEach((t) => t.stop());
    } catch {
      /* ignore */
    }
    try {
      $("#call-remote-video").srcObject = null;
      $("#call-local-video").srcObject = null;
    } catch {
      /* ignore */
    }
  }

  function toggleMute() {
    const c = state.call;
    if (!c || !c.localStream) return;
    c.muted = !c.muted;
    for (const t of c.localStream.getAudioTracks()) t.enabled = !c.muted;
    const btn = $("#call-mute");
    btn.classList.toggle("is-muted", c.muted);
    btn.textContent = c.muted ? "🔇" : "🎙";
    btn.title = c.muted ? "Unmute microphone" : "Mute microphone";
  }

  // ── Call overlay / media-element plumbing ──────────────────────────────────
  function showCallOverlay() {
    const c = state.call;
    if (!c) return;
    $("#call-peer").textContent = displayName(c.peer);
    $("#call-media-label").textContent = c.media === "video" ? "Video call" : "Voice call";
    $("#call-timer").hidden = true;
    attachLocalMedia();
    attachRemoteMedia();
    $("#call-active").hidden = false;
  }

  function hideCallOverlay() {
    $("#call-active").hidden = true;
    const mb = $("#call-mute");
    mb.classList.remove("is-muted");
    mb.textContent = "🎙";
    mb.title = "Mute microphone";
  }

  function showRingingOverlay() {
    const c = state.call;
    if (!c) return;
    $("#ring-peer").textContent = displayName(c.peer);
    $("#ring-media").textContent =
      c.media === "video" ? "Incoming video call" : "Incoming voice call";
    $("#call-ring").hidden = false;
  }

  function hideRingingOverlay() {
    $("#call-ring").hidden = true;
  }

  function attachLocalMedia() {
    const c = state.call;
    if (!c) return;
    const lv = $("#call-local-video");
    if (c.media === "video" && c.localStream) {
      lv.srcObject = c.localStream;
      lv.hidden = false;
      lv.play().catch(() => {}); // autoplay attr isn't always honored in a webview
    } else {
      lv.srcObject = null;
      lv.hidden = true;
    }
  }

  function attachRemoteMedia() {
    const c = state.call;
    if (!c) return;
    // One <video> carries remote audio+video; on a voice call it just plays
    // audio while the avatar covers the empty frame.
    if (c.remoteStream) {
      const rv = $("#call-remote-video");
      rv.srcObject = c.remoteStream;
      rv.play().catch(() => {}); // best-effort; user gesture (the call) already occurred
    }
    $("#call-avatar").hidden = c.media === "video";
  }

  function setCallStatusText(text) {
    const node = $("#call-status");
    if (text == null) {
      node.hidden = true;
      return;
    }
    node.hidden = false;
    node.textContent = text;
  }

  function startDurationTimer() {
    stopDurationTimer();
    const timerEl = $("#call-timer");
    timerEl.hidden = false;
    setCallStatusText(null);
    const tick = () => {
      const c = state.call;
      if (!c || !c.startedAt) return;
      const s = Math.max(0, nowSecs() - c.startedAt);
      const mm = String(Math.floor(s / 60)).padStart(2, "0");
      const ss = String(s % 60).padStart(2, "0");
      timerEl.textContent = `${mm}:${ss}`;
    };
    tick();
    if (state.call) state.call.timerId = setInterval(tick, 1000);
  }

  function stopDurationTimer() {
    if (state.call && state.call.timerId) {
      clearInterval(state.call.timerId);
      state.call.timerId = null;
    }
  }

  // A media bubble: renders inline if we already hold an object URL (our own
  // sent media), otherwise a Download button that fetches + decrypts on click.
  function mediaBubble(m) {
    const wrap = el("div", { class: "bubble " + (m.outgoing ? "out" : "in") });
    if (m.media.caption) wrap.append(el("div", { class: "media-caption", text: m.media.caption }));

    if (m.media.objectUrl) {
      wrap.append(renderMediaEl(m.media.mime, m.media.objectUrl));
    } else {
      const btn = el("button", { class: "btn btn-ghost btn-sm", text: "⬇ Download & view" });
      btn.addEventListener("click", async () => {
        // Dedupe: a re-render can hand out a fresh button for the same message
        // while a download is in flight — guard so we never fetch (and mint an
        // object URL) twice for one blob.
        if (m.media.objectUrl || m.media.loading) return;
        m.media.loading = true;
        btn.disabled = true;
        btn.textContent = "Decrypting…";
        try {
          const out = await safeInvoke("download_and_decrypt_media", {
            eventId: m.media.eventId,
          });
          const mime = out.mime_type || m.media.mime;
          if (!m.media.objectUrl) {
            m.media.objectUrl = URL.createObjectURL(base64ToBlob(out.base64, mime));
          }
          // Re-render from state (not replaceChild on a possibly-detached node)
          // so the inline media lands in the live DOM for whichever screen shows it.
          renderConversation();
          renderCoupleMedia();
        } catch {
          btn.disabled = false;
          btn.textContent = "⬇ Retry";
        } finally {
          m.media.loading = false;
        }
      });
      wrap.append(btn);
    }
    wrap.append(el("span", { class: "bubble-time", text: relTime(m.created_at) }));
    return wrap;
  }

  function onIncomingMedia(p) {
    const key = p.sender || "unknown";
    const list = state.dms.get(key) || [];
    list.push({
      created_at: p.created_at,
      outgoing: false,
      media: { eventId: p.event_id, mime: p.mime_type, caption: p.caption },
    });
    state.dms.set(key, list);
    renderContacts();
    if (state.activeContact === key) renderConversation();
    // The backend normalises the sender to bech32 npub, matching the partner
    // npub the couple panel is keyed by — repaint it when partner media lands.
    if (key === state.partnerNpub) renderCoupleMedia();
    showToast(`New encrypted media from ${shortNpub(key)}`, "info");
  }

  // Encrypt + upload a selected file to `targetPubkey`, then render it locally.
  async function handleAttach(file, targetPubkey) {
    if (!file) return;
    if (!targetPubkey) {
      showToast("No recipient selected", "warn");
      return;
    }
    if (file.size > MAX_MEDIA_BYTES) {
      showToast(
        `"${file.name}" is ${(file.size / 1048576).toFixed(1)} MB — over the 10 MB limit`,
        "warn",
      );
      return;
    }
    const mime = file.type || "application/octet-stream";
    let base64;
    try {
      base64 = await fileToBase64(file);
    } catch {
      showToast("Could not read the file", "error");
      return;
    }
    showToast("Encrypting & uploading…", "info");
    try {
      const dto = await safeInvoke("send_media_bytes", {
        targetPubkey,
        mimeType: mime,
        caption: file.name,
        base64,
      });
      // Optimistic local render straight from the picked file — no round-trip.
      const objectUrl = URL.createObjectURL(file);
      const list = state.dms.get(targetPubkey) || [];
      list.push({
        created_at: dto.created_at || nowSecs(),
        outgoing: true,
        media: { eventId: dto.event_id, mime, caption: file.name, objectUrl },
      });
      state.dms.set(targetPubkey, list);
      if (state.activeContact === targetPubkey) {
        renderContacts();
        renderConversation();
      } else if (document.body.dataset.screen === "app") {
        selectContact(targetPubkey);
      }
      renderCoupleMedia();
      showToast("Encrypted media sent", "success");
    } catch {
      /* toasted */
    }
  }

  function renderCoupleMedia() {
    const box = $("#couple-media");
    if (!box || !state.partnerNpub) return;
    box.innerHTML = "";
    const msgs = state.dms.get(state.partnerNpub) || [];
    for (const m of msgs) if (m.media) box.append(mediaBubble(m));
    box.scrollTop = box.scrollHeight;
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
      // Honest copy: switching workspace only changes the app's mode today —
      // engine disconnect/mesh start-up is not wired yet (AUDIT A1 / M2-4).
      showToast(
        want
          ? "Off-Grid / Travel mode enabled (relay disconnect not yet implemented)"
          : "Back in Base mode",
        "info",
      );
    } catch {
      e.target.checked = !want; // revert the switch on a blocked transition
    }
  }

  // ── Milestone 4: Partner Portal — real Sakha/Sakhi pairing handshake ──────
  //
  // Pairing is a genuine Diffie-Hellman key exchange (`pair_sakha`, backed by
  // `SakhaEngine::pair_with`) between two Nostr public keys — not a client-side
  // token check. A completed pairing is persisted on the backend and survives
  // a relaunch, so a returning couple gets a "Continue" shortcut instead of
  // being asked to paste each other's keys again every session.

  /** Opening the portal decides which face to show: the pairing form (first
   * time, or pairing with someone new) or the "already paired" shortcut. */
  async function openPartnerModal() {
    $("#modal-partner").hidden = false;
    let status = null;
    try {
      status = await safeInvoke("sakha_status", undefined, { silent: true });
    } catch {
      /* vault locked or an older backend without the command — show the form */
    }
    if (status && status.paired) {
      showPairExisting(status);
    } else {
      showPairForm();
    }
  }

  function closePartnerModal() {
    $("#modal-partner").hidden = true;
  }

  function showPairExisting(status) {
    $("#pair-existing-npub").textContent = shortNpub(status.partner_npub || "");
    $("#pair-existing-role").textContent = status.role === "sakhi" ? "Sakhi" : "Sakha";
    $("#pair-existing").hidden = false;
    $("#pair-form").hidden = true;
  }

  function showPairForm() {
    $("#pair-existing").hidden = true;
    $("#pair-form").hidden = false;
    $("#pair-payload").focus();
  }

  /** Re-enter the sandbox as an already-paired partner — no new handshake. */
  async function handlePairContinue() {
    const btn = $("#pair-continue");
    setBusy(btn, true);
    try {
      const status = await safeInvoke("sakha_status", undefined, { silent: true });
      const role = status.role === "sakhi" ? "sakhi" : "sakha";
      const target = role === "sakhi" ? "CoupleSandboxSakhi" : "CoupleSandboxSakha";
      const ws = await safeInvoke("toggle_app_workspace", { target });
      state.coupleRole = role;
      closePartnerModal();
      applyWorkspace(ws);
    } catch {
      /* toasted */
    } finally {
      setBusy(btn, false);
    }
  }

  /** Perform the real pairing handshake, then enter the sandbox. */
  async function handlePair() {
    const payload = $("#pair-payload").value.trim();
    const role = (document.querySelector("input[name=pair-role]:checked") || {}).value || "sakha";
    if (!/^npub1[0-9a-z]+$/i.test(payload)) {
      showToast("Enter your partner's npub public key", "warn");
      return;
    }
    const btn = $("#pair-submit");
    setBusy(btn, true);
    try {
      await safeInvoke("pair_sakha", { partnerPubkey: payload, role });
      const target = role === "sakhi" ? "CoupleSandboxSakhi" : "CoupleSandboxSakha";
      const ws = await safeInvoke("toggle_app_workspace", { target });
      state.coupleRole = role;
      $("#pair-payload").value = "";
      closePartnerModal();
      applyWorkspace(ws);
      showToast("Paired — your shared ledger is ready", "success");
    } catch {
      /* e.g. an invalid key, or blocked because Travel mode is active —
         toasted already */
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

  // ── Hisab-Kitab shared ledger: pairing status, entries, live sync ────────

  function setLedgerFormEnabled(enabled) {
    for (const id of ["ledger-desc", "ledger-amount", "ledger-paid-by", "ledger-add-btn"]) {
      $(`#${id}`).disabled = !enabled;
    }
  }

  function renderLedgerText(text) {
    $("#ledger-status").textContent =
      text && text.trim() ? text : "No entries yet — add the first one below.";
  }

  /** Pull the authoritative pairing state and refresh everything it drives:
   * the partner key (couple media), the entry form's enabled state, and the
   * ledger content itself. Called whenever the Couple Sandbox screen opens. */
  async function refreshSakhaStatus() {
    let status;
    try {
      status = await safeInvoke("sakha_status", undefined, { silent: true });
    } catch {
      return; // older backend without the command, or vault locked
    }
    state.partnerNpub = status.partner_npub || null;
    $("#couple-attach").disabled = !state.partnerNpub;
    setLedgerFormEnabled(!!status.paired);
    renderCoupleMedia();
    if (status.paired) await loadLedger();
    else $("#ledger-status").textContent = "Not yet paired.";
  }

  async function loadLedger() {
    try {
      renderLedgerText(await safeInvoke("sakha_read_ledger", undefined, { silent: true }));
    } catch {
      /* leave whatever was already shown */
    }
  }

  async function handleAddLedgerEntry(e) {
    e.preventDefault();
    const description = $("#ledger-desc").value.trim();
    const paidBy = $("#ledger-paid-by").value.trim();
    const amountInr = parseFloat($("#ledger-amount").value);
    if (!description || !paidBy || !Number.isFinite(amountInr) || amountInr < 0) {
      showToast("Fill in what it was for, the amount, and who paid", "warn");
      return;
    }
    const btn = $("#ledger-add-btn");
    setBusy(btn, true);
    try {
      renderLedgerText(await safeInvoke("sakha_add_entry", { description, amountInr, paidBy }));
      $("#ledger-desc").value = "";
      $("#ledger-amount").value = "";
      $("#ledger-paid-by").value = "";
      $("#ledger-desc").focus();
    } catch {
      /* toasted */
    } finally {
      setBusy(btn, false);
    }
  }

  /** The partner pushed a ledger update over the sync channel — refresh live. */
  function onLedgerUpdated(p) {
    renderLedgerText(p.ledger || "");
    showToast("Your partner updated the shared ledger", "info");
  }

  async function handleSyncLedger() {
    const btn = $("#sync-ledger-btn");
    setBusy(btn, true);
    try {
      await safeInvoke("sync_ledger");
      showToast("Hisab-Kitab ledger synced to your partner", "success");
    } catch {
      /* toasted */
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
        } else if (p.type === "incoming_media") {
          onIncomingMedia(p);
        } else if (p.type === "incoming_call_signal") {
          onCallSignal(p);
        } else if (p.type === "incoming_message_request") {
          onIncomingMessageRequest(p);
        } else if (p.type === "message_status") {
          onMessageStatus(p);
        } else if (p.type === "peer_profile_updated") {
          onPeerProfileUpdated(p);
        } else if (p.type === "ledger_updated") {
          onLedgerUpdated(p);
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
    $("#dm-send").addEventListener("click", handleDmSend);
    $("#dm-input").addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleDmSend();
      }
    });

    // Media attachments (Vault + Couple sandbox)
    $("#dm-attach").addEventListener("click", () => $("#dm-file").click());
    $("#dm-file").addEventListener("change", (e) => {
      const file = e.target.files && e.target.files[0];
      handleAttach(file, state.activeContact);
      e.target.value = "";
    });
    $("#couple-attach").addEventListener("click", () => $("#couple-file").click());
    $("#couple-file").addEventListener("change", (e) => {
      const file = e.target.files && e.target.files[0];
      handleAttach(file, state.partnerNpub);
      e.target.value = "";
    });

    $("#travel-toggle").addEventListener("change", handleTravel);
    $("#partner-btn").addEventListener("click", openPartnerModal);
    $("#partner-cancel").addEventListener("click", closePartnerModal);
    $("#pair-submit").addEventListener("click", handlePair);
    $("#pair-continue").addEventListener("click", handlePairContinue);
    $("#pair-again").addEventListener("click", showPairForm);
    $("#couple-exit").addEventListener("click", exitCouple);
    $("#sync-ledger-btn").addEventListener("click", handleSyncLedger);
    $("#ledger-entry-form").addEventListener("submit", handleAddLedgerEntry);

    // Reply chip + message requests + call settings (Milestone 6)
    $("#dm-reply-cancel").addEventListener("click", clearReply);
    $("#call-settings-btn").addEventListener("click", openTurnModal);
    $("#turn-cancel").addEventListener("click", closeTurnModal);
    $("#turn-save").addEventListener("click", handleSaveTurn);

    // Call overlays (ringing + in-call controls)
    $("#ring-accept").addEventListener("click", acceptIncoming);
    $("#ring-decline").addEventListener("click", declineIncoming);
    $("#call-mute").addEventListener("click", toggleMute);
    $("#call-hangup").addEventListener("click", hangupByUser);

    $("#modal-partner").addEventListener("click", (e) => {
      if (e.target === $("#modal-partner")) closePartnerModal();
    });
    $("#modal-turn").addEventListener("click", (e) => {
      if (e.target === $("#modal-turn")) closeTurnModal();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      if (!$("#modal-partner").hidden) closePartnerModal();
      else if (!$("#modal-turn").hidden) closeTurnModal();
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
    const ICE_DEMO = [
      { urls: ["stun:stun.l.google.com:19302"], username: null, credential: null },
    ];
    // A demo message request so the Requests UI is visible in browser preview;
    // accept/block splice it so the interaction feels real without a backend.
    let mockRequests = [
      {
        peer: "npub1stranger00000000000000000000000000000000000000000",
        last_message: "Hey, saw your Chitthi — mind if we chat? (mock)",
        // mockBackend() runs at module-init time (`const backend = hasTauri ?
        // … : mockBackend()` above), before the `nowSecs` const further down
        // this same scope is initialized — calling it here would be a
        // temporal-dead-zone ReferenceError, so compute the timestamp inline.
        last_at: Math.floor(Date.now() / 1000) - 300,
      },
    ];
    // Local Sakha/Sakhi pairing + ledger state, so the pairing modal and the
    // Couple Sandbox behave believably in browser preview.
    let mockSakha = { paired: false, partnerNpub: null, role: null, ledger: "" };

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
        case "send_dm":
          return {
            id: "mockdm_" + Date.now(),
            peer: args.target,
            content: args.content,
            created_at: nowSecs(),
            outgoing: true,
          };
        case "conversations":
        case "messages_with":
        case "media_with":
        case "list_contacts":
          return [];
        case "current_profile":
          return { npub: "npub1mockdev0identity00000000000000000000000000000000", username: "mockuser" };
        case "extract_payments": {
          const out = [];
          let m;
          re.lastIndex = 0;
          while ((m = re.exec(args.text || "")) !== null)
            out.push({ amount_inr: parseFloat(m[1]), vpa: m[2], uri: `upi://pay?pa=${m[2]}&am=${m[1]}` });
          return out;
        }
        case "pair_sakha":
          mockSakha.paired = true;
          mockSakha.partnerNpub = args.partnerPubkey;
          mockSakha.role = args.role === "sakhi" ? "sakhi" : "sakha";
          return { paired: true, partner_npub: mockSakha.partnerNpub, role: mockSakha.role };
        case "sakha_status":
          return mockSakha.paired
            ? { paired: true, partner_npub: mockSakha.partnerNpub, role: mockSakha.role }
            : { paired: false, partner_npub: null, role: null };
        case "sakha_add_entry": {
          if (!mockSakha.paired) throw "not paired with a partner yet";
          const line = `[mock] ${args.description} | ₹${Number(args.amountInr).toFixed(2)} | paid by ${args.paidBy}`;
          mockSakha.ledger = mockSakha.ledger ? `${mockSakha.ledger}\n${line}` : line;
          return mockSakha.ledger;
        }
        case "sakha_read_ledger":
          return mockSakha.ledger;
        case "sync_ledger":
          if (!mockSakha.paired) throw "no shared secret available — pairing handshake incomplete";
          return "mockledgersync_" + Date.now();
        case "send_media_bytes":
          return {
            event_id: "mockmedia_" + Date.now(),
            url: "https://cdn.hackers.town/mock",
            mime_type: args.mimeType,
            caption: args.caption || "",
            sender: "npub1mockdev0identity00000000000000000000000000000000",
            created_at: nowSecs(),
            size: 0,
          };
        case "download_and_decrypt_media":
          // 1×1 transparent PNG so the preview can render an <img>.
          return {
            mime_type: "image/png",
            base64:
              "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
          };
        // ── Milestone 6: replies / receipts / requests / calls ──────────────
        case "send_dm_reply":
          return {
            id: "mockdm_" + Date.now(),
            peer: args.target,
            content: args.content,
            created_at: nowSecs(),
            outgoing: true,
            status: "sent",
            reply_to: args.replyTo || null,
          };
        case "mark_conversation_read":
          return null;
        case "message_requests":
          return mockRequests.slice();
        case "accept_request":
          mockRequests = mockRequests.filter((r) => r.peer !== args.peer);
          return null;
        case "block_conversation":
          mockRequests = mockRequests.filter((r) => r.peer !== args.peer);
          return null;
        case "call_ice_servers":
          return ICE_DEMO.slice();
        case "set_turn_server":
          return null;
        case "place_call":
          return {
            call_id: "mockcall_" + Date.now(),
            peer: args.peer,
            media: args.media,
            ice_servers: ICE_DEMO.slice(),
          };
        case "send_call_signal":
        case "hangup_call":
          return null;
        case "log_call":
          return {
            id: "mockrec_" + Date.now(),
            peer: args.peer,
            media: args.media,
            incoming: !!args.incoming,
            outcome: args.outcome,
            started_at: args.startedAt || nowSecs(),
            duration_secs: args.durationSecs || 0,
          };
        case "call_history":
          return [];
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
