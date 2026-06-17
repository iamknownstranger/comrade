// Comrade desktop frontend. Talks to the Rust backend over Tauri IPC.
// `withGlobalTauri: true` in tauri.conf.json exposes window.__TAURI__.
const { invoke } = window.__TAURI__.core;

const $ = (id) => document.getElementById(id);

async function renderWorkspaces() {
  const container = $("workspaces");
  container.innerHTML = "";
  const list = await invoke("workspaces");
  for (const ws of list) {
    const btn = document.createElement("button");
    btn.className =
      "w-full text-left rounded-lg px-3 py-2 text-sm transition " +
      (ws.active
        ? "bg-indigo-600 font-medium"
        : "bg-slate-800 hover:bg-slate-700");
    btn.innerHTML = `<div>${ws.label}</div>
      <div class="text-[10px] text-slate-400 mt-0.5">
        relays:${ws.relay_connected ? "on" : "off"} ·
        mesh:${ws.mesh_active ? "on" : "off"} ·
        sandbox:${ws.couple_sandbox ? "on" : "off"}
      </div>`;
    btn.onclick = async () => {
      try {
        await invoke("switch_workspace", { key: ws.key });
      } catch (e) {
        console.warn("transition blocked:", e);
      }
      await renderWorkspaces();
    };
    container.appendChild(btn);
  }
}

async function refreshIdentity() {
  const id = await invoke("current_identity");
  $("identity").textContent = id ? id.npub : "(no identity yet)";
}

$("back-btn").onclick = async () => {
  await invoke("back");
  await renderWorkspaces();
};

$("gen-btn").onclick = async () => {
  const id = await invoke("generate_identity");
  $("identity").textContent = id.npub;
};

$("pay-input").addEventListener("input", async (e) => {
  const result = $("pay-result");
  result.innerHTML = "";
  const intents = await invoke("extract_payments", { text: e.target.value });
  for (const i of intents) {
    const row = document.createElement("div");
    row.className = "rounded bg-emerald-900/40 px-2 py-1";
    row.textContent = `₹${i.amount_inr.toFixed(2)} → ${i.vpa}`;
    result.appendChild(row);
  }
});

// Initial render
renderWorkspaces();
refreshIdentity();
