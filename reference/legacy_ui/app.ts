import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openDialog } from '@tauri-apps/plugin-dialog';

// -----------------------------------------------------------------------------
// Approval modal (HITL)
// -----------------------------------------------------------------------------

interface ApprovalRequiredError {
  kind: 'approval_required';
  request_id: string;
  operation: string;
  summary: string;
  reason_code: string;
}

interface AffectedItem {
  name: string;
  path: string;
}

interface ImpactSummary {
  item_count: number;
  affected_items: AffectedItem[];
  operation_description: string;
  target_path?: string;
}

interface ApprovalRequestedPayload {
  request_id: string;
  caller: string;
  tool_type: string;
  operation: string;
  target: string;
  target_hint: string;
  reason_code: string;
  summary: string;
  risk_hints: string[];
  created_at_ms: number;
  expires_at_ms: number;
  details_redacted: Record<string, unknown>;
  impact_summary?: ImpactSummary;
  danger_zone?: boolean;
}

function getApprovalRequired(err: unknown): ApprovalRequiredError | null {
  const obj = (typeof err === 'object' && err !== null && 'data' in err
    ? (err as { data: unknown }).data
    : err) as ApprovalRequiredError | null;
  return obj?.kind === 'approval_required' ? obj : null;
}

async function showApprovalModal(payload: ApprovalRequestedPayload): Promise<boolean> {
  const callerLabel = payload.caller === 'agent' ? 'Agent' : payload.caller === 'user' ? 'User' : payload.caller;
  const targetLabel = payload.target_hint === 'path' ? 'Path' : payload.target_hint === 'command' ? 'Command' : 'Target';

  const impactHtml = payload.impact_summary
    ? `
      <div style="margin: 12px 0; padding: 12px; background: #252525; border-radius: 6px; border: 1px solid #444;">
        <p style="margin: 0 0 8px; font-size: 13px; font-weight: 600;">Impact summary</p>
        <p style="margin: 0 0 8px; font-size: 13px; color: #c0c0c0;">${payload.impact_summary.operation_description}</p>
        <ul style="margin: 0; padding-left: 20px; font-size: 13px; color: #a0a0a0;">
          ${payload.impact_summary.affected_items
            .map((i) => `<li><strong>${escapeHtml(i.name)}</strong> — ${escapeHtml(i.path)}</li>`)
            .join('')}
        </ul>
        ${payload.impact_summary.target_path ? `<p style="margin: 8px 0 0 0; font-size: 12px; color: #888;">Destination: <code>${escapeHtml(payload.impact_summary.target_path)}</code></p>` : ''}
      </div>
    `
    : '';

  const dangerZoneHtml =
    payload.danger_zone === true
      ? `
      <div style="margin: 12px 0; padding: 12px; background: #332208; border: 1px solid #664; border-radius: 6px;">
        <p style="margin: 0; font-size: 13px; color: #f0ad4e;"><strong>Danger zone</strong></p>
        <p style="margin: 6px 0 0 0; font-size: 12px; color: #c9a227;">This action is irreversible. The item will be permanently deleted with no undo or trash.</p>
      </div>
    `
      : '';

  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.id = 'approval-overlay';
    overlay.style.cssText = `
      position: fixed;
      inset: 0;
      background: rgba(0,0,0,0.5);
      display: flex;
      align-items: center;
      justify-content: center;
      z-index: 1000;
    `;

    const modal = document.createElement('div');
    modal.style.cssText = `
      background: #1e1e1e;
      border-radius: 8px;
      padding: 24px;
      max-width: 480px;
      max-height: 90vh;
      overflow-y: auto;
      box-shadow: 0 4px 24px rgba(0,0,0,0.4);
      font-family: system-ui, -apple-system, sans-serif;
      color: #e0e0e0;
    `;

    let detailsVisible = false;
    const detailsEl = document.createElement('pre');
    detailsEl.style.cssText = `
      margin: 12px 0 0 0;
      padding: 12px;
      background: #2a2a2a;
      border-radius: 4px;
      font-size: 12px;
      white-space: pre-wrap;
      word-break: break-all;
      display: none;
    `;
    detailsEl.textContent = JSON.stringify(payload.details_redacted, null, 2);

    modal.innerHTML = `
      <h3 style="margin: 0 0 16px; font-size: 18px;">Confirm destructive operation</h3>
      <p style="margin: 0 0 4px; font-size: 14px; color: #a0a0a0;">${escapeHtml(payload.summary)}</p>
      ${impactHtml}
      ${dangerZoneHtml}
      <p style="margin: 8px 0 0 0; font-size: 13px; color: #909090;">Caller: <strong>${escapeHtml(callerLabel)}</strong></p>
      <p style="margin: 0 0 4px; font-size: 13px; color: #909090;">${targetLabel}: <code style="font-size: 12px; background: #2a2a2a; padding: 2px 6px; border-radius: 4px;">${escapeHtml(payload.target)}</code></p>
      ${payload.risk_hints.length > 0 ? `<p style="margin: 8px 0 0 0; font-size: 12px; color: #f0ad4e;">Risk: ${escapeHtml(payload.risk_hints.join(', '))}</p>` : ''}
      <div style="display: flex; gap: 12px; justify-content: flex-end; margin-top: 20px; flex-wrap: wrap;">
        <button id="approval-cancel" style="
          padding: 8px 16px;
          background: #333;
          color: #fff;
          border: none;
          border-radius: 4px;
          cursor: pointer;
        ">Cancel</button>
        <button id="approval-view-details" style="
          padding: 8px 16px;
          background: #3a3a3a;
          color: #aaa;
          border: none;
          border-radius: 4px;
          cursor: pointer;
        ">View details</button>
        <button id="approval-accept" style="
          padding: 8px 16px;
          background: #0a84ff;
          color: #fff;
          border: none;
          border-radius: 4px;
          cursor: pointer;
        ">Accept</button>
      </div>
    `;

    modal.appendChild(detailsEl);

    overlay.appendChild(modal);
    document.body.appendChild(overlay);

    const cleanup = () => {
      overlay.remove();
    };

    modal.querySelector('#approval-cancel')!.addEventListener('click', () => {
      cleanup();
      resolve(false);
    });

    modal.querySelector('#approval-view-details')!.addEventListener('click', () => {
      detailsVisible = !detailsVisible;
      (detailsEl as HTMLElement).style.display = detailsVisible ? 'block' : 'none';
      (modal.querySelector('#approval-view-details') as HTMLButtonElement).textContent = detailsVisible ? 'Hide details' : 'View details';
    });

    modal.querySelector('#approval-accept')!.addEventListener('click', () => {
      cleanup();
      resolve(true);
    });
  });
}

function escapeHtml(s: string): string {
  const div = document.createElement('div');
  div.textContent = s;
  return div.innerHTML;
}

const handlingRequestIds = new Set<string>();

async function handleInvokeWithApproval<T>(
  command: string,
  args: Record<string, unknown>
): Promise<T> {
  try {
    return (await invoke(command, args)) as T;
  } catch (err) {
    const approval = getApprovalRequired(err);
    if (approval) {
      handlingRequestIds.add(approval.request_id);
      const payload: ApprovalRequestedPayload = {
        request_id: approval.request_id,
        caller: 'agent',
        tool_type: approval.operation.startsWith('fs_') ? 'fs' : 'shell',
        operation: approval.operation,
        target: approval.summary,
        target_hint: approval.operation.startsWith('fs_') ? 'path' : 'command',
        reason_code: approval.reason_code,
        summary: approval.summary,
        risk_hints: [approval.reason_code],
        created_at_ms: 0,
        expires_at_ms: 300_000,
        details_redacted: { operation: approval.operation, summary: approval.summary },
      };
      const approved = await showApprovalModal(payload);
      handlingRequestIds.delete(approval.request_id);
      const result = await invoke('cmd_approve_and_execute', {
        request_id: approval.request_id,
        approved,
      });
      if (!approved) {
        throw new Error('User denied the operation');
      }
      return result as T;
    }
    throw err;
  }
}

listen<ApprovalRequestedPayload>('security.approval.requested', (event) => {
  const payload = event.payload;
  if (handlingRequestIds.has(payload.request_id)) return;
  handlingRequestIds.add(payload.request_id);
  showApprovalModal(payload).then(async (approved) => {
    try {
      await invoke('cmd_approve_and_execute', {
        request_id: payload.request_id,
        approved,
      });
      if (approved) {
        document.getElementById('app')!.innerText = `Approved and executed: ${payload.summary}`;
      } else {
        document.getElementById('app')!.innerText = `Denied: ${payload.summary}`;
      }
    } catch (e) {
      document.getElementById('app')!.innerText = `Error: ${e}`;
    } finally {
      handlingRequestIds.delete(payload.request_id);
    }
  });
});

// -----------------------------------------------------------------------------
// Test FS (existing flow)
// -----------------------------------------------------------------------------

document.getElementById('test-fs')!.onclick = async () => {
  try {
    const workspaceRoot = process.cwd?.() ?? '/';
    const files = await invoke('cmd_fs_list_dir', {
      workspaceRoot,
      relPath: '.',
      caller: null,
    });
    document.getElementById('app')!.innerText = JSON.stringify(files, null, 2);
  } catch (err) {
    document.getElementById('app')!.innerText = `Error: ${err}`;
  }
};

document.getElementById('test-write-agent')!.onclick = async () => {
  try {
    const workspaceRoot = process.cwd?.() ?? '/';
    await handleInvokeWithApproval('cmd_fs_write_file', {
      workspaceRoot,
      relPath: 'oxcer-test.txt',
      contents: Array.from(new TextEncoder().encode('Hello from Oxcer HITL test')),
      caller: 'agent_orchestrator',
    });
    document.getElementById('app')!.innerText = 'Write completed (approved)';
  } catch (err) {
    document.getElementById('app')!.innerText = `Error: ${err}`;
  }
};

// -----------------------------------------------------------------------------
// Settings screen (Basic + Advanced tabs)
// -----------------------------------------------------------------------------

interface WorkspaceDirectory {
  id: string;
  name: string;
  path: string;
}

interface AdvancedSettings {
  allow_destructive_fs_without_hitl?: boolean;
  allow_agent_write_without_approval?: boolean;
  allow_agent_exec_without_approval?: boolean;
}

interface AppSettings {
  workspace_directories: WorkspaceDirectory[];
  default_model_id: string;
  advanced: AdvancedSettings;
}

function showMainView(): void {
  (document.getElementById('loading') as HTMLElement).style.display = 'none';
  (document.getElementById('settings-view') as HTMLElement).style.display = 'none';
  (document.getElementById('main-view') as HTMLElement).style.display = 'block';
}

function showSettingsView(): void {
  (document.getElementById('loading') as HTMLElement).style.display = 'none';
  (document.getElementById('main-view') as HTMLElement).style.display = 'none';
  (document.getElementById('settings-view') as HTMLElement).style.display = 'block';
}

async function loadSettings(): Promise<AppSettings> {
  return (await invoke('cmd_settings_get')) as AppSettings;
}

async function saveSettings(settings: AppSettings): Promise<void> {
  await invoke('cmd_settings_save', { settings });
}

function renderSettingsScreen(settings: AppSettings): void {
  const container = document.getElementById('settings-container')!;
  const advanced = settings.advanced ?? {};
  const tabStyle = 'padding: 8px 16px; cursor: pointer; border: 1px solid #444; background: #2a2a2a; color: #ccc;';
  const tabActive = 'background: #0a84ff; color: #fff; border-color: #0a84ff;';
  container.innerHTML = `
    <div style="font-family: system-ui, -apple-system, sans-serif; color: #e0e0e0; padding: 16px; max-width: 560px;">
      <div style="display: flex; align-items: center; gap: 12px; margin-bottom: 20px;">
        <button id="settings-back" style="padding: 6px 12px; background: #333; color: #fff; border: none; border-radius: 4px; cursor: pointer;">← Back</button>
        <h2 style="margin: 0; font-size: 20px;">Settings</h2>
      </div>
      <div style="display: flex; gap: 4px; margin-bottom: 20px; border-bottom: 1px solid #444;">
        <button id="tab-basic" style="${tabStyle}">Basic</button>
        <button id="tab-advanced" style="${tabStyle}">Advanced</button>
      </div>
      <div id="panel-basic" style="display: block;">
        <h3 style="margin: 0 0 12px; font-size: 16px;">Workspace directories</h3>
        <p style="margin: 0 0 12px; font-size: 13px; color: #a0a0a0;">Only the selected directory is registered as a workspace. Add folders via the directory picker.</p>
        <ul id="workspace-list" style="list-style: none; padding: 0; margin: 0 0 12px; border: 1px solid #444; border-radius: 4px; min-height: 40px;">
          ${(settings.workspace_directories ?? []).length === 0
    ? '<li style="padding: 12px; color: #666;">No workspaces added yet.</li>'
    : (settings.workspace_directories ?? []).map((w: WorkspaceDirectory) => `
            <li style="padding: 10px 12px; border-bottom: 1px solid #333; display: flex; justify-content: space-between; align-items: center;">
              <span style="font-size: 13px;" title="${w.path}">${w.name}</span>
              <button data-workspace-id="${w.id}" class="ws-remove" style="padding: 4px 10px; background: #522; color: #faa; border: none; border-radius: 4px; cursor: pointer;">Remove</button>
            </li>`).join('')}
        </ul>
        <button id="workspace-add" style="padding: 8px 16px; background: #0a84ff; color: #fff; border: none; border-radius: 4px; cursor: pointer;">Add directory…</button>
        <h3 style="margin: 24px 0 12px; font-size: 16px;">Default model</h3>
        <p style="margin: 0 0 8px; font-size: 13px; color: #a0a0a0;">Stored only; Semantic Router / real model routing will be implemented in a later sprint.</p>
        <select id="default-model" style="padding: 8px 12px; background: #2a2a2a; color: #e0e0e0; border: 1px solid #444; border-radius: 4px; min-width: 200px;">
          <option value="">— Select model —</option>
        </select>
      </div>
      <div id="panel-advanced" style="display: none;">
        <div style="background: #332208; border: 1px solid #664; border-radius: 6px; padding: 14px; margin-bottom: 20px;">
          <p style="margin: 0; font-size: 14px; color: #f0ad4e;"><strong>⚠ Advanced / Dangerous</strong></p>
          <p style="margin: 8px 0 0 0; font-size: 13px; color: #c9a227;">These options reduce safety. All are disabled by default. Only enable if you understand the risks.</p>
        </div>
        <div style="display: flex; flex-direction: column; gap: 12px;">
          <label style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
            <input type="checkbox" id="adv-destructive-fs" ${(advanced.allow_destructive_fs_without_hitl ?? false) ? 'checked' : ''} />
            <span>Allow destructive file operations (delete / rename / move)</span>
          </label>
          <label style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
            <input type="checkbox" id="adv-agent-write" ${(advanced.allow_agent_write_without_approval ?? false) ? 'checked' : ''} />
            <span>Allow agent to write files without per-request approval</span>
          </label>
          <label style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
            <input type="checkbox" id="adv-agent-exec" ${(advanced.allow_agent_exec_without_approval ?? false) ? 'checked' : ''} />
            <span>Allow agent to run shell commands without per-request approval</span>
          </label>
        </div>
        <div id="locked-commands" style="margin-top: 16px; padding: 12px; background: #252525; border-radius: 6px; border: 1px solid #444;">
          <p style="margin: 0 0 8px; font-size: 13px; color: #888;">Locked commands (shown when destructive file operations are off):</p>
          <ul id="locked-commands-list" style="list-style: none; padding: 0; margin: 0; font-size: 13px; color: #a0a0a0;"></ul>
        </div>
      </div>
    </div>`;

  document.getElementById('tab-basic')!.onclick = () => {
    (document.getElementById('panel-basic') as HTMLElement).style.display = 'block';
    (document.getElementById('panel-advanced') as HTMLElement).style.display = 'none';
    (document.getElementById('tab-basic') as HTMLElement).setAttribute('style', tabStyle + tabActive);
    (document.getElementById('tab-advanced') as HTMLElement).setAttribute('style', tabStyle);
  };
  document.getElementById('tab-advanced')!.onclick = () => {
    (document.getElementById('panel-basic') as HTMLElement).style.display = 'none';
    (document.getElementById('panel-advanced') as HTMLElement).style.display = 'block';
    (document.getElementById('tab-basic') as HTMLElement).setAttribute('style', tabStyle);
    (document.getElementById('tab-advanced') as HTMLElement).setAttribute('style', tabStyle + tabActive);
  };
  (document.getElementById('tab-basic') as HTMLElement).setAttribute('style', tabStyle + tabActive);

  document.getElementById('settings-back')!.onclick = () => showMainView();

  invoke('cmd_models_list').then((models: unknown) => {
    const list = models as [string, string][];
    const sel = document.getElementById('default-model') as HTMLSelectElement;
    const value = settings.default_model_id ?? '';
    sel.innerHTML = '';
    list.forEach(([id, label]) => {
      const opt = document.createElement('option');
      opt.value = id;
      opt.textContent = label;
      if (opt.value === value) opt.selected = true;
      sel.appendChild(opt);
    });
  });

  (document.getElementById('default-model') as HTMLSelectElement).onchange = async () => {
    const sel = document.getElementById('default-model') as HTMLSelectElement;
    const next: AppSettings = { ...settings, default_model_id: sel.value };
    await saveSettings(next);
  };

  document.getElementById('workspace-add')!.onclick = async () => {
    try {
      const path = await openDialog({ directory: true, multiple: false });
      if (path) {
        await invoke('cmd_workspace_add', { path });
        const updated = await loadSettings();
        renderSettingsScreen(updated);
      }
    } catch (e) {
      console.error(e);
      alert(String(e));
    }
  };

  container.querySelectorAll('.ws-remove').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const id = (btn as HTMLElement).dataset.workspaceId!;
      try {
        await invoke('cmd_workspace_remove', { id });
        const updated = await loadSettings();
        renderSettingsScreen(updated);
      } catch (e) {
        alert(String(e));
      }
    });
  });

  invoke('get_command_visibility', { context: 'advanced' })
    .then((visibility: Record<string, { disabled_with_explanation?: { message: string }; DisabledWithExplanation?: { message: string } }>) => {
      const listEl = document.getElementById('locked-commands-list');
      if (!listEl) return;
      const labels: Record<string, string> = { fs_delete: 'Delete file', fs_rename: 'Rename file', fs_move: 'Move file' };
      listEl.innerHTML = '';
      for (const [key, val] of Object.entries(visibility)) {
        const msg = val?.disabled_with_explanation?.message ?? val?.DisabledWithExplanation?.message;
        if (msg) {
          const li = document.createElement('li');
          li.style.padding = '6px 0';
          li.innerHTML = `<strong>${labels[key] ?? key}</strong>: ${msg}`;
          listEl.appendChild(li);
        }
      }
      const containerLocked = document.getElementById('locked-commands');
      if (containerLocked) (containerLocked as HTMLElement).style.display = listEl.children.length ? 'block' : 'none';
    })
    .catch(() => {});

  function updateAdvanced(): void {
    const next: AppSettings = {
      ...settings,
      advanced: {
        allow_destructive_fs_without_hitl: (document.getElementById('adv-destructive-fs') as HTMLInputElement).checked,
        allow_agent_write_without_approval: (document.getElementById('adv-agent-write') as HTMLInputElement).checked,
        allow_agent_exec_without_approval: (document.getElementById('adv-agent-exec') as HTMLInputElement).checked,
      },
    };
    saveSettings(next);
  }

  (document.getElementById('adv-destructive-fs') as HTMLInputElement).onchange = async () => {
    const checkbox = document.getElementById('adv-destructive-fs') as HTMLInputElement;
    if (checkbox.checked) {
      checkbox.checked = false;
      const confirmed = await showDestructiveFsConfirmModal();
      if (confirmed) {
        checkbox.checked = true;
        const next: AppSettings = {
          ...settings,
          advanced: { ...settings.advanced, allow_destructive_fs_without_hitl: true },
        };
        await saveSettings(next);
      }
    } else {
      updateAdvanced();
    }
  };
  (document.getElementById('adv-agent-write') as HTMLInputElement).onchange = updateAdvanced;
  (document.getElementById('adv-agent-exec') as HTMLInputElement).onchange = updateAdvanced;
}

function showDestructiveFsConfirmModal(): Promise<boolean> {
  return new Promise((resolve) => {
    const overlay = document.createElement('div');
    overlay.style.cssText = `
      position: fixed; inset: 0; background: rgba(0,0,0,0.5);
      display: flex; align-items: center; justify-content: center; z-index: 2000;
      font-family: system-ui, -apple-system, sans-serif;
    `;
    const modal = document.createElement('div');
    modal.style.cssText = `
      background: #1e1e1e; border-radius: 8px; padding: 24px; max-width: 420px;
      box-shadow: 0 4px 24px rgba(0,0,0,0.4); color: #e0e0e0;
    `;
    modal.innerHTML = `
      <h3 style="margin: 0 0 16px; font-size: 18px;">Enable destructive file operations?</h3>
      <p style="margin: 0 0 16px; font-size: 14px; color: #c0c0c0; line-height: 1.45;">
        The agent will be allowed to delete, rename, and move files in your workspaces.<br>
        These actions can cause data loss. Setting up backups is strongly recommended.
      </p>
      <p style="margin: 0 0 16px; font-size: 14px; color: #a0a0a0; line-height: 1.45;">
        에이전트가 작업 공간 내 파일을 삭제·이름 변경·이동할 수 있게 됩니다. 데이터 손실이 발생할 수 있으며, 백업 설정을 권장합니다.
      </p>
      <label style="display: flex; align-items: center; gap: 8px; margin-bottom: 20px; cursor: pointer;">
        <input type="checkbox" id="destructive-fs-understand-cb" />
        <span>I understand the risks and still want to enable this.</span>
      </label>
      <div style="display: flex; gap: 12px; justify-content: flex-end;">
        <button id="destructive-fs-cancel" style="padding: 8px 16px; background: #333; color: #fff; border: none; border-radius: 4px; cursor: pointer;">Cancel</button>
        <button id="destructive-fs-enable" disabled style="padding: 8px 16px; background: #a33; color: #fff; border: none; border-radius: 4px; cursor: not-allowed; opacity: 0.6;">Enable destructive operations</button>
      </div>
    `;
    overlay.appendChild(modal);
    document.body.appendChild(overlay);

    const cleanup = () => overlay.remove();

    const enableBtn = modal.querySelector('#destructive-fs-enable') as HTMLButtonElement;
    const understandCb = modal.querySelector('#destructive-fs-understand-cb') as HTMLInputElement;

    understandCb.addEventListener('change', () => {
      enableBtn.disabled = !understandCb.checked;
      enableBtn.style.cursor = understandCb.checked ? 'pointer' : 'not-allowed';
      enableBtn.style.opacity = understandCb.checked ? '1' : '0.6';
    });

    enableBtn.addEventListener('click', () => {
      if (!understandCb.checked) return;
      cleanup();
      resolve(true);
    });

    modal.querySelector('#destructive-fs-cancel')!.addEventListener('click', () => {
      cleanup();
      resolve(false);
    });

    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) {
        cleanup();
        resolve(false);
      }
    });
  });
}

document.getElementById('btn-settings')!.onclick = async () => {
  showSettingsView();
  try {
    const settings = await loadSettings();
    renderSettingsScreen(settings);
  } catch (e) {
    document.getElementById('settings-container')!.innerHTML = `<p style="color: #f44;">Failed to load settings: ${e}</p>`;
  }
};

(document.getElementById('loading') as HTMLElement).style.display = 'none';
(document.getElementById('main-view') as HTMLElement).style.display = 'block';
