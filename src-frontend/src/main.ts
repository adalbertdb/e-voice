/**
 * Dashboard entry point.
 *
 * Connects to the e-voice backend via Tauri invoke commands (stubbed for now).
 * Future slices will wire up real IPC calls.
 */

// Stub out Tauri invoke for non-Tauri environments (e.g. plain browser dev).
const invoke: (cmd: string) => Promise<unknown> =
  (window as unknown as { __TAURI__?: { core: { invoke: typeof invoke } } })
    ?.__TAURI__?.core?.invoke ??
  (() => Promise.resolve(null));

async function refreshStatus(): Promise<void> {
  try {
    const model = (await invoke('get_model')) as string | null;
    const reqCount = (await invoke('get_request_count')) as number | null;

    const modelEl = document.getElementById('model-name');
    const reqEl = document.getElementById('req-count');

    if (modelEl) modelEl.textContent = model ?? '—';
    if (reqEl) reqEl.textContent = reqCount != null ? String(reqCount) : '—';
  } catch {
    // Backend not yet connected — display placeholders.
  }
}

// Poll every 2 s for demo purposes; replace with event-driven updates later.
refreshStatus();
setInterval(refreshStatus, 2000);
