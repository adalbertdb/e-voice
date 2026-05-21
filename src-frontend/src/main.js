"use strict";
/**
 * Dashboard entry point.
 *
 * Connects to the e-voice backend via Tauri invoke commands (stubbed for now).
 * Future slices will wire up real IPC calls.
 */
// Stub out Tauri invoke for non-Tauri environments (e.g. plain browser dev).
const invoke = window
    ?.__TAURI__?.core?.invoke ??
    (() => Promise.resolve(null));
async function refreshStatus() {
    try {
        const model = (await invoke('get_model'));
        const reqCount = (await invoke('get_request_count'));
        const modelEl = document.getElementById('model-name');
        const reqEl = document.getElementById('req-count');
        if (modelEl)
            modelEl.textContent = model ?? '—';
        if (reqEl)
            reqEl.textContent = reqCount != null ? String(reqCount) : '—';
    }
    catch {
        // Backend not yet connected — display placeholders.
    }
}
// Poll every 2 s for demo purposes; replace with event-driven updates later.
refreshStatus();
setInterval(refreshStatus, 2000);
