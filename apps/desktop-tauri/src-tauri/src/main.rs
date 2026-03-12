#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Oxcer Tauri launcher — Command Router entry point.
//!
//! ## "Agent = untrusted client" contract
//!
//! **Invariant:** The Agent Orchestrator (and any AI agent) must NEVER call
//! FS/Shell/Web tools directly. All privileged operations MUST go through:
//!
//!   Command Router -> Security Policy Engine -> optional HITL Approval -> tool
//!
//! This module is the ONLY surface for FS/Shell operations. The `invoke_handler`
//! below registers the sole commands (`cmd_fs_*`, `cmd_shell_run`). There are
//! no direct `fs::` or `shell::` Tauri commands — those modules are called
//! internally only AFTER policy evaluation. Agents invoke these commands with
//! `caller: "agent_orchestrator"`; the policy engine enforces stricter rules
//! (e.g. write/exec -> REQUIRE_APPROVAL) for agents.
//!
//! Oxcer's primary UI is a native Swift app. The Tauri backend currently uses a
//! hidden window but can be evolved into a tray app or pure daemon if needed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use http::{header::CONTENT_TYPE, Response, StatusCode};
use tauri::menu::{MenuBuilder, MenuItem, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_fs;
use uuid::Uuid;

use oxcer_core::agent_session_log::AgentSessionLog;
use oxcer_core::fs;
use oxcer_core::llm_metrics::{
    cost_usd as llm_cost_usd, estimate_tokens_from_chars, provider_for_model,
};
use oxcer_core::network::{
    anthropic_client::{self, AnthropicMessagesRequest},
    gemini_client::{self, GeminiChatRequest},
    grok_client::{self, GrokChatRequest},
    openai_client::{self, OpenAIChatRequest},
    HttpClient, HttpError, NetworkTool,
};
use oxcer_core::orchestrator::{
    next_action, run_first_step, OrchestratorAction, SessionState, StepResult,
};
use oxcer_core::plugins::{
    build_capability_registry, load_plugins_from_dir_with_telemetry, plugin_rules_from_descriptors,
    shell_plugins_to_command_specs,
};
use oxcer_core::prompt_sanitizer::{self, ScrubbingError};
use oxcer_core::security::policy_config::{load_from_yaml, merge_rules};
use oxcer_core::security::policy_engine::init_policy_with_config;
use oxcer_core::security::policy_engine::{
    evaluate, Operation, PolicyCaller, PolicyDecision, PolicyDecisionKind, PolicyRequest,
    PolicyTarget, ToolType,
};
use oxcer_core::semantic_router::{category_for_log, strategy_for_log, RouterInput};
use oxcer_core::shell;
use oxcer_core::telemetry::{log_event, LogEvent, LogMetrics};

use oxcer::event_log;
use oxcer::router;
use oxcer::router::{
    get_destructive_command_visibility, to_requested_payload, ApprovalRequestedPayload,
    CommandVisibilityContext, PendingApprovalsStore, PendingOperation, RouterError,
};
use oxcer::settings::{
    get_effective_fs_policy as settings_get_effective_fs_policy, is_forbidden_workspace_path,
    load as settings_load, log_destructive_setting_change as settings_log_destructive_change,
    save as settings_save, to_workspace_roots, AppSettings, EffectiveFsPolicy, WorkspaceDirectory,
};
use oxcer::setup::{
    complete_setup as setup_complete, get_setup_status as setup_get_status,
    start_model_download as setup_start_download,
};

/// If session cost exceeds the configured threshold, emit a cost_threshold_exceeded LogEvent
/// and a Launcher notification (once per session). Sprint 8 §6.
fn check_cost_threshold_and_alert(app: &AppHandle, session_id: &str, session_cost_usd: f64) {
    let threshold = app
        .try_state::<Mutex<AppSettings>>()
        .and_then(|state| {
            state
                .lock()
                .ok()
                .map(|guard| guard.observability.max_session_cost_usd)
        })
        .unwrap_or(0.5);
    if session_cost_usd <= threshold {
        return;
    }
    let alerted = app
        .try_state::<SessionCostAlertedStore>()
        .map(|s| s.has_alerted(session_id))
        .unwrap_or(false);
    if alerted {
        return;
    }
    if let Some(store) = app.try_state::<SessionCostAlertedStore>() {
        store.mark_alerted(session_id);
    }
    if let Ok(app_config_dir) = app.path().app_config_dir() {
        let details = serde_json::json!({ "threshold_usd": threshold });
        let _ = log_event(
            &app_config_dir,
            session_id,
            None,
            "agent",
            "orchestrator",
            "cost_threshold_exceeded",
            Some("alert"),
            LogMetrics {
                cost_usd: Some(session_cost_usd),
                ..Default::default()
            },
            details,
        );
    }
    let msg = format!(
        "This session exceeded ${:.2} in LLM cost. Consider tightening context or routing rules.",
        threshold
    );
    let _ = app.emit(
        "metrics.cost_threshold_exceeded",
        serde_json::json!({
            "session_id": session_id,
            "session_cost_usd": session_cost_usd,
            "threshold_usd": threshold,
            "message": msg,
        }),
    );
}

/// Helper to get effective FS policy from app state (for config gates).
fn effective_fs_policy_from_app(app: &AppHandle) -> EffectiveFsPolicy {
    app.try_state::<Mutex<AppSettings>>()
        .map(|state| settings_get_effective_fs_policy(&state.lock().expect("settings lock")))
        .unwrap_or_else(|| EffectiveFsPolicy {
            allowed_workspaces: vec![],
            destructive_operations_enabled: false,
        })
}

/// Helper to build the FS context from the running application handle and current settings.
fn build_fs_context(app: &AppHandle) -> fs::AppFsContext {
    let app_config_dir = app
        .path()
        .app_config_dir()
        .expect("app_config_dir should be available");

    let workspace_roots = app
        .try_state::<Mutex<AppSettings>>()
        .map(|state| {
            let guard = state.lock().expect("settings lock");
            to_workspace_roots(&guard.workspace_directories)
        })
        .unwrap_or_default();

    fs::AppFsContext {
        app_config_dir,
        workspace_roots,
    }
}

const WORKSPACE_OUTSIDE_MSG: &str =
    "This path is outside your configured workspaces. Please add a workspace in Settings.";

const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Oxcer Guardrails Dashboard</title>
<style>
body{font-family:system-ui;color:#e0e0e0;background:#1a1a1a;margin:0;padding:20px;}
#toast{position:fixed;bottom:20px;right:20px;background:#333;color:#e0e0e0;padding:12px 20px;border-radius:8px;max-width:360px;display:none;}
.tabs{display:flex;gap:8px;margin-bottom:16px;border-bottom:1px solid #333;}
.tabs button{padding:8px 16px;background:transparent;color:#a0a0a0;border:none;cursor:pointer;border-bottom:2px solid transparent;}
.tabs button.active{color:#e0e0e0;border-bottom-color:#4a9eff;}
.panel{display:none;}
.panel.active{display:block;}
#sessionsList{list-style:none;padding:0;margin:0;}
#sessionsList li{padding:12px;margin:4px 0;background:#252525;border-radius:6px;cursor:pointer;display:flex;justify-content:space-between;align-items:center;}
#sessionsList li:hover{background:#2a2a2a;}
#sessionsList li.selected{outline:1px solid #4a9eff;}
#sessionTimeline{display:none;margin-top:16px;}
#timelineFilters{display:flex;gap:12px;margin-bottom:12px;flex-wrap:wrap;}
#timelineFilters input,#timelineFilters select{padding:6px 10px;background:#252525;border:1px solid #333;border-radius:4px;color:#e0e0e0;}
#timelineTable{width:100%;border-collapse:collapse;font-size:13px;}
#timelineTable th{text-align:left;padding:8px;background:#252525;color:#a0a0a0;}
#timelineTable td{padding:8px;border-bottom:1px solid #252525;}
#timelineTable tr:hover{background:#252525;}
#timelineTable tr.expanded td{background:#1e2a33;}
.detailsJson{font-family:monospace;font-size:11px;white-space:pre-wrap;word-break:break-all;max-height:200px;overflow:auto;padding:8px;background:#0d1117;}
.metrics{color:#8b949e;}
.setup-box,.api-warning-box{background:#252525;border-radius:8px;padding:16px;margin:12px 0;border:1px solid #333;}
.api-warning-box{border-left:4px solid #e67e22;}
.setup-progress{margin:8px 0;color:#8b949e;}
</style>
</head>
<body>
<h1>Oxcer Guardrails Dashboard</h1>
<div class="tabs">
  <button type="button" class="tab active" data-panel="dashboard">Dashboard</button>
  <button type="button" class="tab" data-panel="setup">Setup</button>
  <button type="button" class="tab" data-panel="sessions">Recent Sessions</button>
</div>
<div id="panel-dashboard" class="panel active">
  <p style="color:#a0a0a0;">Add workspaces in Settings. Destructive operations require explicit enabling.</p>
  <div id="externalApiWarning" class="api-warning-box" style="margin-top:16px;"><strong>External AI APIs</strong><p id="externalApiWarningText" style="margin:8px 0 0;white-space:pre-wrap;"></p><p style="margin:8px 0 0;color:#a0a0a0;font-size:13px;">This warning appears whenever you configure external API keys. For sensitive data, use local-only mode.</p></div>
</div>
<div id="panel-setup" class="panel">
  <div class="setup-box">
    <h3 style="margin-top:0;">Local LLM setup</h3>
    <p>Oxcer uses a local model (Phi-3-small) so your data can stay on this device. Download the model once to enable local-only mode.</p>
    <p id="setupStatus" class="setup-progress">Checking…</p>
    <button type="button" id="setupDownloadBtn" style="display:none;padding:8px 16px;background:#4a9eff;color:#fff;border:none;border-radius:6px;cursor:pointer;">Download model</button>
    <p id="setupProgress" class="setup-progress" style="display:none;"></p>
    <div id="setupProfileChoice" style="display:none;margin-top:16px;">
      <p><strong>Choose mode:</strong></p>
      <label><input type="radio" name="llm_profile" value="local-only" checked> Local only (recommended for privacy)</label><br>
      <label><input type="radio" name="llm_profile" value="hybrid"> Local + cloud (hybrid)</label>
      <button type="button" id="setupCompleteBtn" style="margin-top:8px;padding:8px 16px;background:#2ea043;color:#fff;border:none;border-radius:6px;cursor:pointer;">Complete setup</button>
    </div>
  </div>
</div>
<div id="panel-sessions" class="panel">
  <p style="color:#a0a0a0;">Per-session telemetry from logs. Select a session to view the timeline.</p>
  <ul id="sessionsList"></ul>
  <div id="sessionTimeline">
    <div id="timelineFilters">
      <select id="filterComponent"><option value="">All components</option><option value="semantic_router">semantic_router</option><option value="llm_client">llm_client</option><option value="security">security</option><option value="orchestrator">orchestrator</option></select>
      <select id="filterDecision"><option value="">All decisions</option><option value="allow">allow</option><option value="deny">deny</option><option value="approval_required">approval_required</option><option value="approve">approve</option><option value="success">success</option><option value="error">error</option></select>
      <input type="text" id="filterText" placeholder="Search details..." style="min-width:180px;">
    </div>
    <table id="timelineTable"><thead><tr><th>Time</th><th>Component</th><th>Action</th><th>Decision</th><th>Metrics</th><th></th></tr></thead><tbody id="timelineBody"></tbody></table>
  </div>
</div>
<div id="toast"></div>
<script>
(function(){
var invoke=window.__TAURI__?.core?.invoke;if(!invoke)return;
function toast(m){var e=document.getElementById('toast');e.textContent=m;e.style.display='block';setTimeout(function(){e.style.display='none';},6000);}
window.__TAURI__?.event?.listen?.('security.destructive_op_executed',function(ev){toast(ev.payload?.summary||'');}).catch(function(){});
window.__TAURI__?.event?.listen?.('metrics.cost_threshold_exceeded',function(ev){toast(ev.payload?.message||'Session exceeded LLM cost threshold.');}).catch(function(){});
function showPanel(id){document.querySelectorAll('.panel').forEach(function(p){p.classList.remove('active');});document.querySelectorAll('.tabs button').forEach(function(b){b.classList.remove('active');});var p=document.getElementById('panel-'+id);if(p)p.classList.add('active');var b=document.querySelector('.tabs button[data-panel="'+id+'"]');if(b)b.classList.add('active');}
document.querySelectorAll('.tab').forEach(function(btn){btn.addEventListener('click',function(){showPanel(btn.dataset.panel);if(btn.dataset.panel==='sessions')loadSessions();if(btn.dataset.panel==='setup')loadSetupStatus();});});
invoke('get_external_api_warning').then(function(t){var e=document.getElementById('externalApiWarningText');if(e)e.textContent=t;}).catch(function(){});
function loadSetupStatus(){invoke('get_setup_status').then(function(s){var statusEl=document.getElementById('setupStatus');var downloadBtn=document.getElementById('setupDownloadBtn');var progressEl=document.getElementById('setupProgress');var profileDiv=document.getElementById('setupProfileChoice');var completeBtn=document.getElementById('setupCompleteBtn');if(s.setup_complete){statusEl.textContent='Setup complete. Profile: '+s.profile;downloadBtn.style.display='none';profileDiv.style.display='none';}else if(s.needs_local_model){statusEl.textContent='Local model not found. Download required (approx. 2GB).';downloadBtn.style.display='inline-block';downloadBtn.onclick=function(){downloadBtn.disabled=true;progressEl.style.display='block';progressEl.textContent='Starting download…';invoke('start_model_download').then(function(){}).catch(function(e){progressEl.textContent='Error: '+e;downloadBtn.disabled=false;});};}else{statusEl.textContent='Local model present. Choose your mode and complete setup.';profileDiv.style.display='block';completeBtn.onclick=function(){var profile=document.querySelector('input[name=llm_profile]:checked');invoke('complete_setup',profile?profile.value:'local-only').then(function(){loadSetupStatus();toast('Setup complete');}).catch(function(e){toast('Error: '+e);});};}}).catch(function(e){document.getElementById('setupStatus').textContent='Error: '+e;});}
window.__TAURI__?.event?.listen?.('llm_download_progress',function(ev){var p=ev.payload;var el=document.getElementById('setupProgress');if(el)el.textContent=(p&&p.file_name?p.file_name:'')+' '+(p&&p.bytes_downloaded!=null?Math.round(p.bytes_downloaded/1e6)+' MB':'')+(p&&p.total_bytes?(/'+Math.round(p.total_bytes/1e6)+' MB'):'');}).catch(function(){});
window.__TAURI__?.event?.listen?.('llm_download_complete',function(ev){var ok=ev.payload&&ev.payload.success;var el=document.getElementById('setupProgress');var btn=document.getElementById('setupDownloadBtn');if(btn)btn.disabled=false;if(ok){if(el)el.textContent='Download complete.';toast('Model download complete');loadSetupStatus();}else{if(el)el.textContent='Download failed: '+(ev.payload&&ev.payload.error||'Unknown error');toast('Download failed');}}).catch(function(){});
function shortId(s){if(!s)return'';return s.length>12?s.slice(0,8)+'…':s;}
function formatTime(ts){if(!ts)return'';try{var d=new Date(ts);return d.toLocaleString();}catch(e){return ts;}}
var allEvents=[];
var selectedSessionId=null;
function loadSessions(){invoke('list_sessions').then(function(summaries){var ul=document.getElementById('sessionsList');ul.innerHTML='';summaries.forEach(function(s){var li=document.createElement('li');li.dataset.sessionId=s.session_id;li.innerHTML='<span><strong>'+shortId(s.session_id)+'</strong> '+formatTime(s.start_timestamp)+'</span><span>'+s.total_cost_usd.toFixed(4)+' USD · '+s.tool_calls_count+' tools · '+s.approvals_count+' ok / '+s.denies_count+' deny'+(s.success?' ✓':' ✗')+'</span>';li.addEventListener('click',function(){selectedSessionId=s.session_id;document.querySelectorAll('#sessionsList li').forEach(function(x){x.classList.toggle('selected',x.dataset.sessionId===s.session_id);});loadSessionLog(s.session_id);});ul.appendChild(li);});}).catch(function(e){toast('Failed to list sessions: '+e);});}
function loadSessionLog(sessionId){invoke('load_session_log',{sessionId:sessionId}).then(function(events){allEvents=events;renderTimeline();document.getElementById('sessionTimeline').style.display='block';}).catch(function(e){toast('Failed to load log: '+e);});}
function renderTimeline(){var comp=document.getElementById('filterComponent').value;var dec=document.getElementById('filterDecision').value;var text=(document.getElementById('filterText').value||'').toLowerCase();var tbody=document.getElementById('timelineBody');tbody.innerHTML='';var events=allEvents.filter(function(e){if(comp&&e.component!==comp)return false;if(dec&&e.decision!==dec)return false;if(text&&JSON.stringify(e.details).toLowerCase().indexOf(text)===-1)return false;return true;});events.forEach(function(ev){var metrics=[];if(ev.metrics.tokens_in!=null)metrics.push('in:'+ev.metrics.tokens_in);if(ev.metrics.tokens_out!=null)metrics.push('out:'+ev.metrics.tokens_out);if(ev.metrics.latency_ms!=null)metrics.push(ev.metrics.latency_ms+'ms');if(ev.metrics.cost_usd!=null)metrics.push('$'+ev.metrics.cost_usd.toFixed(4));var tr=document.createElement('tr');tr.innerHTML='<td>'+formatTime(ev.timestamp)+'</td><td>'+ev.component+'</td><td>'+ev.action+'</td><td>'+(ev.decision||'—')+'</td><td class="metrics">'+metrics.join(' ')+'</td><td><button type="button">Details</button></td>';var btn=tr.querySelector('button');var detailsRow=document.createElement('tr');detailsRow.style.display='none';var td=document.createElement('td');td.colSpan=6;td.className='detailsJson';detailsRow.appendChild(td);btn.addEventListener('click',function(){var open=detailsRow.style.display!=='none';detailsRow.style.display=open?'none':'table-row';td.textContent=open?'':JSON.stringify(ev.details,null,2);tr.classList.toggle('expanded',!open);});tbody.appendChild(tr);tbody.appendChild(detailsRow);});}
document.getElementById('filterComponent').addEventListener('change',renderTimeline);
document.getElementById('filterDecision').addEventListener('change',renderTimeline);
document.getElementById('filterText').addEventListener('input',renderTimeline);
})();
</script>
</body>
</html>"#;

/// Resolve workspace_root to (ctx, workspace_id). No implicit defaults.
/// - Fails if no workspaces are configured.
/// - Fails if workspace_root does not match any registered workspace.
fn ctx_and_workspace_id(
    app: &AppHandle,
    workspace_root: &str,
) -> Result<(fs::AppFsContext, String), RouterError> {
    let ctx = build_fs_context(app);
    if ctx.workspace_roots.is_empty() {
        return Err(RouterError::PolicyDenied {
            reason_code: "NO_WORKSPACES".to_string(),
            message: WORKSPACE_OUTSIDE_MSG.to_string(),
        });
    }
    let path_buf = PathBuf::from(workspace_root);
    let canonical = path_buf.canonicalize().ok();
    let id_opt = ctx.workspace_roots.iter().find_map(|w| {
        if w.path == path_buf {
            Some(w.id.clone())
        } else if let Some(ref can) = canonical {
            w.path
                .canonicalize()
                .ok()
                .filter(|cw| cw == can)
                .map(|_| w.id.clone())
        } else {
            None
        }
    });
    if let Some(id) = id_opt {
        return Ok((ctx, id));
    }
    Err(RouterError::PolicyDenied {
        reason_code: "WORKSPACE_OUTSIDE_SCOPE".to_string(),
        message: WORKSPACE_OUTSIDE_MSG.to_string(),
    })
}

fn fs_caller_from_policy(c: PolicyCaller) -> fs::FsCaller {
    match c {
        PolicyCaller::Ui => fs::FsCaller::Ui,
        PolicyCaller::AgentOrchestrator => fs::FsCaller::Agent,
        PolicyCaller::InternalSystem => fs::FsCaller::ShellTool,
    }
}

/// In-memory store for agent orchestrator sessions (per session_id).
/// Used so the frontend can resume after executing a tool call or after approval.
pub struct AgentSessionStore(Mutex<HashMap<String, SessionState>>);

impl AgentSessionStore {
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
    fn insert(&self, session_id: String, state: SessionState) {
        self.0
            .lock()
            .expect("session store lock")
            .insert(session_id, state);
    }
    fn get(&self, session_id: &str) -> Option<SessionState> {
        self.0
            .lock()
            .expect("session store lock")
            .get(session_id)
            .cloned()
    }
    fn remove(&self, session_id: &str) -> Option<SessionState> {
        self.0
            .lock()
            .expect("session store lock")
            .remove(session_id)
    }
}

impl Default for AgentSessionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-session LLM token and cost totals for session_summary (Sprint 8 §4.2).
#[derive(Clone, Debug, Default)]
pub struct SessionLlmTotals {
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub cost_usd: f64,
}

/// In-memory store for session LLM metrics (tokens_in, tokens_out, cost_usd).
pub struct SessionLlmMetricsStore(Mutex<HashMap<String, SessionLlmTotals>>);

impl SessionLlmMetricsStore {
    pub fn new() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
    pub fn add(&self, session_id: &str, tokens_in: u32, tokens_out: u32, cost_usd: f64) {
        let mut g = self.0.lock().expect("session llm metrics lock");
        let e = g.entry(session_id.to_string()).or_default();
        e.tokens_in += tokens_in;
        e.tokens_out += tokens_out;
        e.cost_usd += cost_usd;
    }
    /// Current totals for a session (without removing). Used for cost-threshold check.
    pub fn get_totals(&self, session_id: &str) -> Option<SessionLlmTotals> {
        self.0
            .lock()
            .expect("session llm metrics lock")
            .get(session_id)
            .cloned()
    }
    pub fn take(&self, session_id: &str) -> Option<SessionLlmTotals> {
        self.0
            .lock()
            .expect("session llm metrics lock")
            .remove(session_id)
    }
}

/// Sessions that have already triggered a cost-threshold alert (avoid duplicate notifications).
pub struct SessionCostAlertedStore(Mutex<std::collections::HashSet<String>>);

impl SessionCostAlertedStore {
    pub fn new() -> Self {
        Self(Mutex::new(std::collections::HashSet::new()))
    }
    pub fn mark_alerted(&self, session_id: &str) {
        self.0
            .lock()
            .expect("cost alerted lock")
            .insert(session_id.to_string());
    }
    pub fn has_alerted(&self, session_id: &str) -> bool {
        self.0
            .lock()
            .expect("cost alerted lock")
            .contains(session_id)
    }
}

impl Default for SessionCostAlertedStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for SessionLlmMetricsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns (workspace_id, workspace_root) for the first configured workspace, if any.
fn default_workspace_from_app(app: &AppHandle) -> (Option<String>, Option<String>) {
    app.try_state::<Mutex<AppSettings>>()
        .and_then(|state| {
            let guard = state.lock().ok()?;
            let w = guard.workspace_directories.first()?;
            Some((Some(w.id.clone()), Some(w.path.clone())))
        })
        .unwrap_or((None, None))
}

fn shell_caller_from_policy(c: PolicyCaller) -> shell::ShellCaller {
    match c {
        PolicyCaller::Ui => shell::ShellCaller::Ui,
        PolicyCaller::AgentOrchestrator => shell::ShellCaller::Agent,
        PolicyCaller::InternalSystem => shell::ShellCaller::System,
    }
}

/// Emit structured telemetry for a policy evaluation (Sprint 8 §3.1).
fn emit_policy_evaluate_telemetry(
    app: &AppHandle,
    session_id: &str,
    request: &PolicyRequest,
    decision: &PolicyDecision,
    workspace_id: Option<&str>,
) {
    let Ok(app_config_dir) = app.path().app_config_dir() else {
        return;
    };
    let caller_str = match request.caller {
        PolicyCaller::Ui => "ui",
        PolicyCaller::AgentOrchestrator => "agent",
        PolicyCaller::InternalSystem => "system",
    };
    let decision_str = match decision.decision {
        PolicyDecisionKind::Allow => "allow",
        PolicyDecisionKind::Deny => "deny",
        PolicyDecisionKind::RequireApproval => "approval_required",
    };
    let tool_name = match request.tool_type {
        ToolType::Fs => "fs",
        ToolType::Shell => "shell",
        ToolType::Agent => "agent",
        ToolType::Web => "web",
        ToolType::Other => "other",
    };
    let operation_str = match request.operation {
        Operation::Read => "read",
        Operation::Write => "write",
        Operation::Delete => "delete",
        Operation::Rename => "rename",
        Operation::Move => "move",
        Operation::Chmod => "chmod",
        Operation::Exec => "exec",
    };
    let data_sensitivity_level = request
        .content_sensitivity
        .as_ref()
        .map(|s| format!("{:?}", s.level).to_lowercase());
    let details = serde_json::json!({
        "tool": tool_name,
        "operation": operation_str,
        "workspace_id": workspace_id.unwrap_or(""),
        "rule_id": decision.reason_code.as_str(),
        "rule_reason": decision.reason_code.as_str(),
        "data_sensitivity_level": data_sensitivity_level,
    });
    let _ = log_event(
        &app_config_dir,
        session_id,
        None,
        caller_str,
        "security",
        "policy_evaluate",
        Some(decision_str),
        LogMetrics::default(),
        details,
    );
}

/// Emit structured telemetry for an approval request (Sprint 8 §3.2).
fn emit_approval_request_telemetry(app: &AppHandle, payload: &ApprovalRequestedPayload) {
    let Ok(app_config_dir) = app.path().app_config_dir() else {
        return;
    };
    let details = serde_json::json!({
        "request_id": payload.request_id,
        "impact_summary": payload.impact_summary,
        "danger_zone": payload.danger_zone,
    });
    let _ = log_event(
        &app_config_dir,
        "",
        None,
        &payload.caller,
        "security",
        "approval_request",
        Some("approval_required"),
        LogMetrics::default(),
        details,
    );
}

/// FS list_dir — Agent MUST use this; never call fs:: directly.
#[tauri::command]
fn cmd_fs_list_dir(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    caller: Option<String>,
) -> Result<Vec<fs::DirEntryMetadata>, RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace {
        id: workspace_id.clone(),
    };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
        ..Default::default()
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, Some(&workspace_id));
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Access denied by policy".to_string(),
        });
    }

    fs::fs_list_dir(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id },
        &rel_path,
    )
    .map_err(RouterError::from)
}

/// FS read_file — Agent MUST use this; never call fs:: directly.
#[tauri::command]
fn cmd_fs_read_file(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    caller: Option<String>,
) -> Result<fs::FsReadResult, RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace {
        id: workspace_id.clone(),
    };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Read,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
        ..Default::default()
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, Some(&workspace_id));
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Access denied by policy".to_string(),
        });
    }

    fs::fs_read_file(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id },
        &rel_path,
    )
    .map_err(RouterError::from)
}

/// FS write_file — Agent MUST use this; never call fs:: directly.
#[tauri::command]
fn cmd_fs_write_file(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    contents: Vec<u8>,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());

    let canonical_path = std::path::Path::new(&workspace_root)
        .join(&rel_path)
        .display()
        .to_string();

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Write,
        target: PolicyTarget::FsPath {
            canonical_path: canonical_path.clone(),
        },
        ..Default::default()
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, None);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Write denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Write to {}", rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Write,
            PolicyTarget::FsPath {
                canonical_path: canonical_path.clone(),
            },
            PendingOperation::FsWrite {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                contents: contents.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_write".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    fs::fs_write_file(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace { id: workspace_id },
        &rel_path,
        &contents,
    )
    .map_err(RouterError::from)
}

const DESTRUCTIVE_DISABLED_MSG: &str = "Destructive file operations are disabled in Settings.";

/// Emit event when a high-risk FS op completes (for toast feedback).
fn emit_destructive_op_executed(app: &AppHandle, summary: &str) {
    let _ = app.emit(
        "security.destructive_op_executed",
        serde_json::json!({
            "summary": summary,
            "unlocked": true,
        }),
    );
}

/// FS delete — Agent MUST use this. Gated by config.
/// Agent never executes delete immediately; all agent delete requests require explicit user approval.
#[tauri::command]
fn cmd_fs_delete(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy = effective_fs_policy_from_app(&app);
    if !policy.destructive_operations_enabled {
        return Err(RouterError::ConfigDisabled {
            message: DESTRUCTIVE_DISABLED_MSG.to_string(),
        });
    }

    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace {
        id: workspace_id.clone(),
    };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Delete,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
        ..Default::default()
    };

    // Agent must NEVER execute delete without explicit user approval.
    if policy_caller == PolicyCaller::AgentOrchestrator {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Delete {}", rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Delete,
            request.target,
            PendingOperation::FsDelete {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
            },
            "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        if let Ok(app_config_dir) = app.path().app_config_dir() {
            let _ = event_log::append(
                &app_config_dir,
                "destructive_approval.requested",
                Some(&workspace_id),
                Some(&serde_json::json!({
                    "operation": "fs_delete",
                    "request_id": request_id,
                    "rel_path": rel_path,
                    "summary": summary
                })),
            );
        }
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_delete".to_string(),
            summary,
            reason_code: "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
        });
    }

    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, Some(&workspace_id));
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Delete denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Delete {}", rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Delete,
            request.target,
            PendingOperation::FsDelete {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_delete".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    fs::fs_remove_file(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace {
            id: workspace_id.clone(),
        },
        &rel_path,
    )
    .map_err(RouterError::from)?;
    emit_destructive_op_executed(
        &app,
        &format!(
            "Deleted {} in \"{}/\". (Destructive operations enabled in Settings.)",
            rel_path, workspace_id
        ),
    );
    Ok(())
}

/// FS rename — Agent MUST use this. Gated by config.
/// Agent never executes rename immediately; all agent rename requests require explicit user approval.
#[tauri::command]
fn cmd_fs_rename(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    new_rel_path: String,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy = effective_fs_policy_from_app(&app);
    if !policy.destructive_operations_enabled {
        return Err(RouterError::ConfigDisabled {
            message: DESTRUCTIVE_DISABLED_MSG.to_string(),
        });
    }

    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;

    let base = fs::BaseDirKind::Workspace {
        id: workspace_id.clone(),
    };
    let normalized = fs::normalize_and_resolve(&ctx, &base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Rename,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
        ..Default::default()
    };

    if policy_caller == PolicyCaller::AgentOrchestrator {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Rename {} -> {}", rel_path, new_rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Rename,
            request.target,
            PendingOperation::FsRename {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                new_rel_path: new_rel_path.clone(),
            },
            "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        if let Ok(app_config_dir) = app.path().app_config_dir() {
            let _ = event_log::append(
                &app_config_dir,
                "destructive_approval.requested",
                Some(&workspace_id),
                Some(&serde_json::json!({
                    "operation": "fs_rename",
                    "request_id": request_id,
                    "rel_path": rel_path,
                    "new_rel_path": new_rel_path,
                    "summary": summary
                })),
            );
        }
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_rename".to_string(),
            summary,
            reason_code: "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
        });
    }

    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, Some(&workspace_id));
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Rename denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Rename {} -> {}", rel_path, new_rel_path);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Rename,
            request.target,
            PendingOperation::FsRename {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                new_rel_path: new_rel_path.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_rename".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    fs::fs_rename(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace {
            id: workspace_id.clone(),
        },
        &rel_path,
        &new_rel_path,
    )
    .map_err(RouterError::from)?;
    emit_destructive_op_executed(
        &app,
        &format!(
            "Renamed {} -> {} in \"{}/\". (Destructive operations enabled in Settings.)",
            rel_path, new_rel_path, workspace_id
        ),
    );
    Ok(())
}

/// FS move — Agent MUST use this. Gated by config.
/// Agent never executes move immediately; all agent move requests require explicit user approval.
#[tauri::command]
fn cmd_fs_move(
    app: AppHandle,
    workspace_root: String,
    rel_path: String,
    dest_workspace_root: String,
    dest_rel_path: String,
    caller: Option<String>,
) -> Result<(), RouterError> {
    let policy = effective_fs_policy_from_app(&app);
    if !policy.destructive_operations_enabled {
        return Err(RouterError::ConfigDisabled {
            message: DESTRUCTIVE_DISABLED_MSG.to_string(),
        });
    }

    let policy_caller = router::parse_caller(caller.as_deref());
    let (ctx, src_workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
    let (_, dest_workspace_id) = ctx_and_workspace_id(&app, &dest_workspace_root)?;

    let src_base = fs::BaseDirKind::Workspace {
        id: src_workspace_id.clone(),
    };
    let normalized = fs::normalize_and_resolve(&ctx, &src_base, &rel_path)?;

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Fs,
        operation: Operation::Move,
        target: PolicyTarget::FsPath {
            canonical_path: normalized.abs_path.display().to_string(),
        },
        ..Default::default()
    };

    if policy_caller == PolicyCaller::AgentOrchestrator {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!(
            "Move {} -> {}/{}",
            rel_path, dest_workspace_root, dest_rel_path
        );
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Move,
            request.target,
            PendingOperation::FsMove {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                dest_workspace_root: dest_workspace_root.clone(),
                dest_rel_path: dest_rel_path.clone(),
            },
            "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        if let Ok(app_config_dir) = app.path().app_config_dir() {
            let _ = event_log::append(
                &app_config_dir,
                "destructive_approval.requested",
                Some(&src_workspace_id),
                Some(&serde_json::json!({
                    "operation": "fs_move",
                    "request_id": request_id,
                    "rel_path": rel_path,
                    "dest_workspace_root": dest_workspace_root,
                    "dest_rel_path": dest_rel_path,
                    "summary": summary
                })),
            );
        }
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_move".to_string(),
            summary,
            reason_code: "AGENT_DESTRUCTIVE_REQUIRES_APPROVAL".to_string(),
        });
    }

    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, Some(&src_workspace_id));
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Move denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!(
            "Move {} -> {}/{}",
            rel_path, dest_workspace_root, dest_rel_path
        );
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Fs,
            Operation::Move,
            request.target,
            PendingOperation::FsMove {
                workspace_root: workspace_root.clone(),
                rel_path: rel_path.clone(),
                dest_workspace_root: dest_workspace_root.clone(),
                dest_rel_path: dest_rel_path.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "fs_move".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    fs::fs_move(
        fs_caller_from_policy(policy_caller),
        &ctx,
        fs::BaseDirKind::Workspace {
            id: src_workspace_id.clone(),
        },
        &rel_path,
        fs::BaseDirKind::Workspace {
            id: dest_workspace_id.clone(),
        },
        &dest_rel_path,
    )
    .map_err(RouterError::from)?;
    emit_destructive_op_executed(
        &app,
        &format!(
            "Moved {} -> \"{}/{}\". (Destructive operations enabled in Settings.)",
            rel_path, dest_workspace_id, dest_rel_path
        ),
    );
    Ok(())
}

/// Shell run — Agent MUST use this; never call shell:: directly.
#[tauri::command]
fn cmd_shell_run(
    app: AppHandle,
    workspace_root: String,
    command_id: String,
    params: serde_json::Value,
    caller: Option<String>,
) -> Result<shell::ShellResult, RouterError> {
    let policy_caller = router::parse_caller(caller.as_deref());

    let request = PolicyRequest {
        caller: policy_caller,
        tool_type: ToolType::Shell,
        operation: Operation::Exec,
        target: PolicyTarget::ShellCommand {
            command_id: command_id.clone(),
            normalized_command: None,
        },
        ..Default::default()
    };
    let decision = evaluate(request.clone());
    router::log_policy_decision(&request, &decision);
    emit_policy_evaluate_telemetry(&app, "", &request, &decision, None);
    if decision.decision == PolicyDecisionKind::Deny {
        return Err(RouterError::PolicyDenied {
            reason_code: decision.reason_code.as_str().to_string(),
            message: "Command denied by policy".to_string(),
        });
    }
    if decision.decision == PolicyDecisionKind::RequireApproval {
        let request_id = Uuid::new_v4().to_string();
        let summary = format!("Execute command: {}", command_id);
        let store = app.state::<PendingApprovalsStore>();
        let record = store.create_record(
            request_id.clone(),
            policy_caller,
            ToolType::Shell,
            Operation::Exec,
            PolicyTarget::ShellCommand {
                command_id: command_id.clone(),
                normalized_command: None,
            },
            PendingOperation::ShellRun {
                workspace_root: workspace_root.clone(),
                command_id: command_id.clone(),
                params: params.clone(),
            },
            decision.reason_code.as_str().to_string(),
            summary.clone(),
        );
        store.insert(record.clone());
        let payload = to_requested_payload(&record);
        app.emit("security.approval.requested", &payload).ok();
        emit_approval_request_telemetry(&app, &payload);
        return Err(RouterError::ApprovalRequired {
            request_id,
            operation: "shell_run".to_string(),
            summary,
            reason_code: decision.reason_code.as_str().to_string(),
        });
    }

    let (fs_ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
    let ctx = shell::ShellContext {
        workspace_roots: fs_ctx.workspace_roots,
        default_workspace_id: workspace_id,
    };
    let catalog = app
        .try_state::<Arc<shell::CommandCatalog>>()
        .map(|state| state.inner().clone())
        .unwrap_or_else(|| Arc::new(shell::default_catalog()));
    shell::shell_run(
        shell_caller_from_policy(policy_caller),
        &ctx,
        catalog.as_ref(),
        &command_id,
        params,
    )
    .map_err(RouterError::from)
}

/// Execute a pending approval request after user confirms in the HITL modal.
/// On Allow: marks record APPROVED, resumes original command execution.
/// On Deny: marks DENIED, returns error.
/// Destructive (delete/rename/move) requests and decisions are logged to the event log.
#[tauri::command]
fn cmd_approve_and_execute(
    app: AppHandle,
    request_id: String,
    approved: bool,
) -> Result<serde_json::Value, RouterError> {
    let store = app.state::<PendingApprovalsStore>();
    let record = store
        .take(&request_id)
        .ok_or_else(|| RouterError::PolicyDenied {
            reason_code: "EXPIRED_OR_UNKNOWN".to_string(),
            message: "Approval request expired or not found".to_string(),
        })?;

    let is_destructive = matches!(
        record.operation_payload,
        PendingOperation::FsDelete { .. }
            | PendingOperation::FsRename { .. }
            | PendingOperation::FsMove { .. }
    );
    let op_name = match &record.operation_payload {
        PendingOperation::FsDelete { .. } => "fs_delete",
        PendingOperation::FsRename { .. } => "fs_rename",
        PendingOperation::FsMove { .. } => "fs_move",
        PendingOperation::FsWrite { .. } => "fs_write",
        PendingOperation::ShellRun { .. } => "shell_run",
    };
    let workspace_id_for_telemetry = match &record.operation_payload {
        PendingOperation::FsWrite { workspace_root, .. }
        | PendingOperation::FsDelete { workspace_root, .. }
        | PendingOperation::FsRename { workspace_root, .. }
        | PendingOperation::FsMove { workspace_root, .. }
        | PendingOperation::ShellRun { workspace_root, .. } => workspace_root.as_str(),
    };

    if is_destructive {
        if let Ok(dir) = app.path().app_config_dir() {
            let event_type = if approved {
                "destructive_approval.approved"
            } else {
                "destructive_approval.denied"
            };
            let _ = event_log::append(
                &dir,
                event_type,
                None,
                Some(&serde_json::json!({
                    "request_id": request_id,
                    "operation": op_name,
                    "summary": record.summary
                })),
            );
        }
    }

    // Approval decision telemetry (Sprint 8 §3.2): latency and approve/deny.
    let approval_time_ms = record.created_at.elapsed().as_millis() as u64;
    if let Ok(app_config_dir) = app.path().app_config_dir() {
        let decision_str = if approved { "approve" } else { "deny" };
        let details = serde_json::json!({
            "request_id": request_id,
            "operation": op_name,
            "workspace_id": workspace_id_for_telemetry,
            "danger_zone": is_destructive,
        });
        let _ = log_event(
            &app_config_dir,
            "",
            None,
            "ui",
            "security",
            "approval_decision",
            Some(decision_str),
            LogMetrics {
                latency_ms: Some(approval_time_ms),
                ..Default::default()
            },
            details,
        );
    }

    if !approved {
        return Err(RouterError::PolicyDenied {
            reason_code: "USER_DENIED".to_string(),
            message: "User denied the operation".to_string(),
        });
    }

    match record.operation_payload {
        PendingOperation::FsWrite {
            workspace_root,
            rel_path,
            contents,
        } => {
            let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            fs::fs_write_file(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace { id: workspace_id },
                &rel_path,
                &contents,
            )
            .map_err(RouterError::from)?;
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::FsDelete {
            workspace_root,
            rel_path,
        } => {
            let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            fs::fs_remove_file(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace {
                    id: workspace_id.clone(),
                },
                &rel_path,
            )
            .map_err(RouterError::from)?;
            emit_destructive_op_executed(
                &app,
                &format!(
                    "Deleted {} in \"{}/\". (Destructive operations enabled in Settings.)",
                    rel_path, workspace_id
                ),
            );
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::FsRename {
            workspace_root,
            rel_path,
            new_rel_path,
        } => {
            let (ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            fs::fs_rename(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace {
                    id: workspace_id.clone(),
                },
                &rel_path,
                &new_rel_path,
            )
            .map_err(RouterError::from)?;
            emit_destructive_op_executed(
                &app,
                &format!(
                    "Renamed {} -> {} in \"{}/\". (Destructive operations enabled in Settings.)",
                    rel_path, new_rel_path, workspace_id
                ),
            );
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::FsMove {
            workspace_root,
            rel_path,
            dest_workspace_root,
            dest_rel_path,
        } => {
            let (ctx, src_workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            let (_, dest_workspace_id) = ctx_and_workspace_id(&app, &dest_workspace_root)?;
            fs::fs_move(
                fs::FsCaller::Agent,
                &ctx,
                fs::BaseDirKind::Workspace {
                    id: src_workspace_id,
                },
                &rel_path,
                fs::BaseDirKind::Workspace {
                    id: dest_workspace_id.clone(),
                },
                &dest_rel_path,
            )
            .map_err(RouterError::from)?;
            emit_destructive_op_executed(
                &app,
                &format!(
                    "Moved {} -> \"{}/{}\". (Destructive operations enabled in Settings.)",
                    rel_path, dest_workspace_id, dest_rel_path
                ),
            );
            Ok(serde_json::json!({ "success": true }))
        }
        PendingOperation::ShellRun {
            workspace_root,
            command_id,
            params,
        } => {
            let (fs_ctx, workspace_id) = ctx_and_workspace_id(&app, &workspace_root)?;
            let ctx = shell::ShellContext {
                workspace_roots: fs_ctx.workspace_roots,
                default_workspace_id: workspace_id,
            };
            let catalog = app
                .try_state::<Arc<shell::CommandCatalog>>()
                .map(|state| state.inner().clone())
                .unwrap_or_else(|| Arc::new(shell::default_catalog()));
            let result = shell::shell_run(
                shell::ShellCaller::Agent,
                &ctx,
                catalog.as_ref(),
                &command_id,
                params,
            )
            .map_err(RouterError::from)?;
            Ok(serde_json::json!({
                "success": true,
                "stdout": result.stdout,
                "stderr": result.stderr,
                "exit_code": result.exit_code,
                "duration_ms": result.duration_ms
            }))
        }
    }
}

// -----------------------------------------------------------------------------
// Settings commands (for Settings screen)
// -----------------------------------------------------------------------------

/// Setup wizard: current status (needs_local_model, setup_complete, profile).
#[tauri::command]
fn get_setup_status(app: AppHandle) -> Result<oxcer::setup::SetupStatus, String> {
    setup_get_status(&app)
}

/// Setup wizard: start downloading local model in background. Listen for llm_download_progress and llm_download_complete.
#[tauri::command]
fn start_model_download(app: AppHandle) -> Result<(), String> {
    setup_start_download(app)
}

/// Setup wizard: mark setup complete and persist LLM profile (local-only or hybrid).
#[tauri::command]
fn complete_setup(app: AppHandle, profile: String) -> Result<(), String> {
    setup_complete(&app, profile)
}

/// External API warning text. Show above API key inputs whenever the user edits external API settings; cannot be hidden.
#[tauri::command]
fn get_external_api_warning() -> &'static str {
    oxcer::setup::EXTERNAL_API_WARNING
}

#[tauri::command]
fn cmd_settings_get(app: AppHandle) -> Result<AppSettings, String> {
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let settings = state.lock().expect("settings lock").clone();
    Ok(settings)
}

#[tauri::command]
fn cmd_settings_save(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let prev = state.lock().expect("settings lock").clone();
    let from = prev.advanced.allow_destructive_fs_without_hitl;
    let to = settings.advanced.allow_destructive_fs_without_hitl;
    settings_save(&app_config_dir, &settings)?;
    if from != to {
        let _ = settings_log_destructive_change(&app_config_dir, from, to);
        let event_type = if to {
            "security.destructive_fs.enabled"
        } else {
            "security.destructive_fs.disabled"
        };
        let _ = event_log::append(
            &app_config_dir,
            event_type,
            None,
            Some(&serde_json::json!({ "from": from, "to": to })),
        );
    }
    *state.lock().expect("settings lock") = settings;
    Ok(())
}

/// Opens native directory picker; returns selected path or null if cancelled.
#[tauri::command]
fn cmd_dialog_open_directory(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::FilePath;

    let path: Option<FilePath> = app.dialog().file().blocking_pick_folder();

    let path = match path {
        Some(file_path) => match file_path.into_path() {
            Ok(pb) => Some(pb.display().to_string()),
            Err(_) => None,
        },
        None => None,
    };

    Ok(path)
}

#[tauri::command]
fn cmd_workspace_add(app: AppHandle, path: String) -> Result<(), String> {
    let path_buf = PathBuf::from(&path);
    if !path_buf.is_dir() {
        return Err("Path is not a directory".to_string());
    }
    if is_forbidden_workspace_path(&path_buf) {
        return Err(
            "This directory cannot be used as a workspace (home or parent of home)".to_string(),
        );
    }
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let mut guard = state.lock().expect("settings lock");
    let canonical = path_buf.canonicalize().map_err(|e| e.to_string())?;
    let path_str = canonical.display().to_string();
    if guard.workspace_directories.iter().any(|w| {
        PathBuf::from(&w.path)
            .canonicalize()
            .as_ref()
            .map(|p| p == &canonical)
            .unwrap_or(false)
    }) {
        return Err("This workspace is already added".to_string());
    }
    let name = path_buf
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Workspace")
        .to_string();
    let id = uuid::Uuid::new_v4().to_string();
    guard.workspace_directories.push(WorkspaceDirectory {
        id: id.clone(),
        name: name.clone(),
        path: path_str.clone(),
    });
    settings_save(&app_config_dir, &*guard)?;
    let _ = event_log::append(
        &app_config_dir,
        "workspace_added",
        Some(&id),
        Some(&serde_json::json!({ "name": name, "root_path": path_str })),
    );
    Ok(())
}

#[tauri::command]
fn cmd_workspace_remove(app: AppHandle, id: String) -> Result<(), String> {
    oxcer::commands::workspace_cleanup_on_delete(&app, &id)
}

/// Returns effective FS policy for the Security Policy Engine.
/// - allowed_workspaces: list of root paths
/// - destructive_operations_enabled: from config
#[tauri::command]
fn get_effective_fs_policy(app: AppHandle) -> Result<EffectiveFsPolicy, String> {
    let state = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?;
    let settings = state.lock().expect("settings lock").clone();
    Ok(settings_get_effective_fs_policy(&settings))
}

/// Returns config.json as JSON (workspaces, default_model, fs options). SSOT for dashboard.
#[tauri::command]
fn get_config(app: AppHandle) -> Result<serde_json::Value, String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let config_path = app_config_dir.join("config.json");
    let config = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => {
            return Ok(serde_json::json!({
                "security": { "destructive_fs": { "enabled": false } },
                "workspaces": [],
                "model": { "default_id": "gemini-2.5-flash" },
                "observability": { "max_session_cost_usd": 0.5 }
            }))
        }
    };
    serde_json::from_str(&config).map_err(|e| e.to_string())
}

/// Returns visibility for destructive commands (delete/rename/move) so UI can hide or show disabled with explanation.
/// context: "main" = command palette (hide when off), "advanced" = Settings advanced (show disabled with message).
#[tauri::command]
fn get_command_visibility(
    app: AppHandle,
    context: CommandVisibilityContext,
) -> Result<std::collections::HashMap<String, oxcer::router::CommandVisibility>, String> {
    let destructive = app
        .try_state::<Mutex<AppSettings>>()
        .ok_or_else(|| "Settings not initialized".to_string())?
        .lock()
        .expect("settings lock")
        .advanced
        .allow_destructive_fs_without_hitl;
    Ok(get_destructive_command_visibility(destructive, context))
}

/// Returns plugin agent tool capabilities for the Semantic Router (Sprint 9).
#[tauri::command]
fn cmd_plugin_capabilities(
    app: AppHandle,
) -> Result<Vec<oxcer_core::plugins::ToolCapability>, String> {
    let store = app
        .try_state::<Mutex<oxcer_core::plugins::CapabilityRegistry>>()
        .ok_or_else(|| "Capability registry not initialized".to_string())?;
    let reg = store.lock().map_err(|e| e.to_string())?;
    Ok(reg.list().to_vec())
}

/// Returns available model options for the default-model dropdown (id + display name). Sprint 5 spec.
/// Stored selection is persisted to model.default_id only; Semantic Router / real model routing in a later sprint.
#[tauri::command]
fn cmd_models_list() -> Vec<(String, String)> {
    vec![
        (String::new(), "— Select model —".to_string()),
        (
            "gemini-2.5-flash".to_string(),
            "Gemini 2.5 Flash (Default)".to_string(),
        ),
        ("gemini-2.5-pro".to_string(), "Gemini 2.5 Pro".to_string()),
        (
            "gemini-1.5-flash".to_string(),
            "Gemini 1.5 Flash".to_string(),
        ),
        ("gpt-4.1-mini".to_string(), "GPT-4.1 Mini".to_string()),
        ("gpt-4o-mini".to_string(), "GPT-4o Mini".to_string()),
        (
            "claude-3.5-sonnet-latest".to_string(),
            "Claude 3.5 Sonnet (Latest)".to_string(),
        ),
        ("grok-4.1-fast".to_string(), "Grok 4.1 Fast".to_string()),
        ("grok-3-mini".to_string(), "Grok 3 Mini".to_string()),
    ]
}

/// Invoke LLM (Gemini, OpenAI, Anthropic, Grok) with per-call telemetry (Sprint 8 §4.1).
#[tauri::command]
async fn cmd_llm_invoke(
    app: AppHandle,
    session_id: String,
    model_id: String,
    task: String,
    strategy: Option<String>,
) -> Result<serde_json::Value, String> {
    use std::time::Instant;

    let start = Instant::now();
    let provider = provider_for_model(&model_id);
    let tokens_in = estimate_tokens_from_chars(&task);
    let strategy_str = strategy.as_deref().unwrap_or("");

    let endpoint_short = match provider {
        "openai" | "grok" => "chat.completions",
        "anthropic" => "messages",
        "gemini" => "generateContent",
        _ => "invoke",
    };

    let (response_text, tokens_out, err_msg) = match provider {
        "gemini" => {
            let api_key = std::env::var("GOOGLE_API_KEY")
                .or_else(|_| std::env::var("GEMINI_API_KEY"))
                .map_err(|_| "GOOGLE_API_KEY or GEMINI_API_KEY not set".to_string())?;
            let client = HttpClient::for_tool(NetworkTool::Gemini).map_err(|e| e.message)?;
            let request = GeminiChatRequest {
                contents: vec![serde_json::json!({"parts": [{"text": task}]})],
                generation_config: None,
            };
            match gemini_client::call_gemini_chat(&client, &model_id, &api_key, &request).await {
                Ok(r) => {
                    let text = r
                        .candidates
                        .as_ref()
                        .and_then(|c| c.first())
                        .and_then(|c| c.get("content"))
                        .and_then(|c| c.get("parts"))
                        .and_then(|p| p.as_array())
                        .and_then(|p| p.first())
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    (text.to_string(), estimate_tokens_from_chars(text), None)
                }
                Err(e) => (String::new(), 0, Some(e.message)),
            }
        }
        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY")
                .map_err(|_| "OPENAI_API_KEY not set".to_string())?;
            let client = HttpClient::for_tool(NetworkTool::OpenAI).map_err(|e| e.message)?;
            let request = OpenAIChatRequest {
                model: model_id.clone(),
                messages: vec![serde_json::json!({"role": "user", "content": task})],
            };
            match openai_client::call_openai_chat(&client, &request, &api_key).await {
                Ok(r) => {
                    let text = r
                        .choices
                        .as_ref()
                        .and_then(|c| c.first())
                        .and_then(|c| c.get("message"))
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    (text.to_string(), estimate_tokens_from_chars(text), None)
                }
                Err(e) => (String::new(), 0, Some(e.message)),
            }
        }
        "anthropic" => {
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .map_err(|_| "ANTHROPIC_API_KEY not set".to_string())?;
            let client = HttpClient::for_tool(NetworkTool::Anthropic).map_err(|e| e.message)?;
            let request = AnthropicMessagesRequest {
                model: model_id.clone(),
                max_tokens: 4096,
                messages: vec![serde_json::json!({"role": "user", "content": task})],
            };
            match anthropic_client::call_anthropic_messages(&client, &request, &api_key).await {
                Ok(r) => {
                    let text = r
                        .content
                        .as_ref()
                        .and_then(|c| c.first())
                        .and_then(|c| c.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    (text.to_string(), estimate_tokens_from_chars(text), None)
                }
                Err(e) => (String::new(), 0, Some(e.message)),
            }
        }
        "grok" => {
            let api_key =
                std::env::var("XAI_API_KEY").map_err(|_| "XAI_API_KEY not set".to_string())?;
            let client = HttpClient::for_tool(NetworkTool::Grok).map_err(|e| e.message)?;
            let request = GrokChatRequest {
                model: model_id.clone(),
                messages: vec![serde_json::json!({"role": "user", "content": task})],
            };
            match grok_client::call_grok_chat(&client, &request, &api_key).await {
                Ok(r) => {
                    let text = r
                        .choices
                        .as_ref()
                        .and_then(|c| c.first())
                        .and_then(|c| c.get("message"))
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    (text.to_string(), estimate_tokens_from_chars(text), None)
                }
                Err(e) => (String::new(), 0, Some(e.message)),
            }
        }
        _ => {
            return Err("Unsupported model (provider not openai/gemini/anthropic/grok)".to_string())
        }
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    match &err_msg {
        None => {
            let cost = llm_cost_usd(provider, &model_id, tokens_in, tokens_out);
            if let Ok(app_config_dir) = app.path().app_config_dir() {
                let details = serde_json::json!({
                    "provider": provider,
                    "model": model_id,
                    "endpoint": endpoint_short,
                    "strategy": strategy_str,
                    "error": serde_json::Value::Null,
                });
                let _ = log_event(
                    &app_config_dir,
                    &session_id,
                    None,
                    "agent",
                    "llm_client",
                    "invoke",
                    Some("success"),
                    LogMetrics {
                        tokens_in: Some(tokens_in),
                        tokens_out: Some(tokens_out),
                        latency_ms: Some(latency_ms),
                        cost_usd: Some(cost),
                    },
                    details,
                );
            }
            if let Some(store) = app.try_state::<SessionLlmMetricsStore>() {
                store.add(&session_id, tokens_in, tokens_out, cost);
                if let Some(totals) = store.get_totals(&session_id) {
                    check_cost_threshold_and_alert(&app, &session_id, totals.cost_usd);
                }
            }
        }
        Some(msg) => {
            if let Ok(app_config_dir) = app.path().app_config_dir() {
                let details = serde_json::json!({
                    "provider": provider,
                    "model": model_id,
                    "endpoint": endpoint_short,
                    "strategy": strategy_str,
                    "error": msg,
                });
                let _ = log_event(
                    &app_config_dir,
                    &session_id,
                    None,
                    "agent",
                    "llm_client",
                    "invoke",
                    Some("error"),
                    LogMetrics {
                        tokens_in: Some(tokens_in),
                        tokens_out: Some(0),
                        latency_ms: Some(latency_ms),
                        cost_usd: None,
                    },
                    details,
                );
            }
        }
    }

    match err_msg {
        None => Ok(serde_json::json!({ "text": response_text })),
        Some(e) => Err(e),
    }
}

/// Scrubbing pipeline for LLM calls: run the central scrubber on the combined payload before sending to any provider.
/// Returns the scrubbed string to use in the request, or an error if ≥50% was redacted (caller must not call the LLM).
/// Audit entry is written to logs/scrubbing.log for observability.
#[tauri::command]
fn cmd_scrub_payload_for_llm(
    app: AppHandle,
    session_id: Option<String>,
    raw_payload: String,
    workspace_root: Option<String>,
) -> Result<String, String> {
    let mut opts = oxcer_core::data_sensitivity::ClassifierOptions::default();
    opts.normalize_paths = workspace_root.is_some();
    opts.workspace_root = workspace_root;
    let sid = session_id.as_deref().unwrap_or("");
    let (result, audit_entry) =
        prompt_sanitizer::scrub_for_llm_call_audit(&raw_payload, &opts, sid);
    if let Ok(app_config_dir) = app.path().app_config_dir() {
        let _ = oxcer::scrubbing_log::append(&app_config_dir, &audit_entry);
    }
    result.map_err(|e| match e {
        ScrubbingError::TooMuchSensitiveData { message } => message,
        ScrubbingError::NeverSendToLlm { message, .. } => message,
    })
}

/// Agent step: run Semantic Router + Orchestrator; return next action (ToolCall, Complete, or AwaitingApproval).
/// The frontend executes tool intents via existing commands (cmd_fs_*, cmd_shell_run) with caller "agent_orchestrator",
/// then passes the result back as last_result and calls this again. All tool execution goes through Command Router -> Policy -> Approval.
#[tauri::command]
fn cmd_agent_step(
    app: AppHandle,
    session_id: String,
    task: String,
    input: RouterInput,
    last_result: Option<StepResult>,
) -> Result<OrchestratorAction, String> {
    let store = app.state::<AgentSessionStore>();
    let (default_ws_id, default_ws_root) = default_workspace_from_app(&app);
    let context_workspace_id = input.context.workspace_id.clone();

    let action = if let (Some(result), Some(session)) =
        (last_result.as_ref(), store.get(&session_id))
    {
        let out = next_action(session, Some(result.clone())).map_err(|e| e.to_string())?;
        match &out {
            OrchestratorAction::ToolCall { session: s, .. }
            | OrchestratorAction::AwaitingApproval { session: s, .. } => {
                store.insert(session_id.clone(), s.clone())
            }
            OrchestratorAction::Complete { session: s, .. } => {
                store.remove(&session_id);
            }
        }
        out
    } else if last_result.is_none() {
        let capabilities = app
            .try_state::<Mutex<oxcer_core::plugins::CapabilityRegistry>>()
            .and_then(|state| state.lock().ok().map(|guard| guard.list().to_vec()));
        let input_with_task = RouterInput {
            task_description: task.clone(),
            capabilities: capabilities.or(input.capabilities),
            ..input
        };
        let action = run_first_step(
            session_id.clone(),
            input_with_task,
            default_ws_id.clone(),
            default_ws_root.clone(),
        )
        .map_err(|e| e.to_string())?;

        // Semantic Router classification telemetry (Sprint 8): before any LLM calls.
        let session_ref = match &action {
            OrchestratorAction::ToolCall { session, .. }
            | OrchestratorAction::AwaitingApproval { session, .. }
            | OrchestratorAction::Complete { session, .. } => session,
        };
        if let (Some(router_output), Ok(app_config_dir)) = (
            session_ref.router_output.as_ref(),
            app.path().app_config_dir(),
        ) {
            let input_length_chars = task.len();
            let tokens_in_approx = (input_length_chars / 4).max(1) as u32;
            let selected_model = app.try_state::<Mutex<AppSettings>>().and_then(|state| {
                state.lock().ok().and_then(|guard| {
                    let id = guard.default_model_id.clone();
                    if id.is_empty() {
                        None
                    } else {
                        Some(id)
                    }
                })
            });
            let details = serde_json::json!({
                "category": category_for_log(router_output.category),
                "strategy": strategy_for_log(router_output.strategy),
                "flags": {
                    "requires_high_risk_approval": router_output.flags.requires_high_risk_approval,
                    "allow_model_tools_mix": router_output.flags.allow_model_tools_mix,
                },
                "input_length_chars": input_length_chars,
                "selected_model": selected_model.as_deref().unwrap_or(""),
            });
            let _ = log_event(
                &app_config_dir,
                &session_id,
                None,
                "agent",
                "semantic_router",
                "classify",
                Some("ok"),
                LogMetrics {
                    tokens_in: Some(tokens_in_approx),
                    ..Default::default()
                },
                details,
            );
        }

        match &action {
            OrchestratorAction::ToolCall { session: s, .. }
            | OrchestratorAction::AwaitingApproval { session: s, .. } => {
                store.insert(session_id.clone(), s.clone())
            }
            OrchestratorAction::Complete { .. } => {}
        }
        action
    } else {
        return Err(
            "Missing session for this session_id; start a new run without last_result.".to_string(),
        );
    };

    // Persist agent session log when complete (for explainability and evaluation).
    if let OrchestratorAction::Complete { session, .. } = &action {
        if let (Some(router_decision), Ok(app_config_dir)) =
            (session.router_output.as_ref(), app.path().app_config_dir())
        {
            let workspace_id = context_workspace_id
                .as_deref()
                .or(default_ws_id.as_deref())
                .unwrap_or("");
            let selected_model = app
                .try_state::<Mutex<AppSettings>>()
                .and_then(|state| {
                    state
                        .lock()
                        .ok()
                        .map(|guard| guard.default_model_id.clone())
                })
                .filter(|s| !s.is_empty());
            let log = AgentSessionLog::from_completed_session(
                &session_id,
                &session.task_description,
                workspace_id,
                router_decision,
                selected_model.as_deref(),
                &session.tool_traces,
                session.accumulated_response.as_deref(),
            );
            let _ = oxcer::agent_sessions::append_session_log(&app_config_dir, &log);

            // Session-level outcome for metrics (Sprint 8): success vs error.
            let outcome = session
                .accumulated_response
                .as_deref()
                .map_or("success", |r| {
                    if r.starts_with("Error:") {
                        "error"
                    } else {
                        "success"
                    }
                });
            let details = serde_json::json!({
                "outcome": outcome,
                "category": category_for_log(router_decision.category),
                "strategy": strategy_for_log(router_decision.strategy),
                "selected_model": selected_model.as_deref().unwrap_or(""),
            });
            let _ = log_event(
                &app_config_dir,
                &session_id,
                None,
                "agent",
                "orchestrator",
                "session_complete",
                Some(outcome),
                LogMetrics::default(),
                details,
            );

            // Session LLM summary (Sprint 8 §4.2): tokens and cost for the session.
            if let Some(llm_store) = app.try_state::<SessionLlmMetricsStore>() {
                if let Some(totals) = llm_store.take(&session_id) {
                    check_cost_threshold_and_alert(&app, &session_id, totals.cost_usd);
                    let tool_total = session.tool_traces.len();
                    let tool_ok = session
                        .tool_traces
                        .iter()
                        .filter(|t| {
                            !t.result_summary
                                .as_deref()
                                .map_or(false, |s| s.starts_with("Error"))
                        })
                        .count();
                    let tool_success_rate = if tool_total > 0 {
                        (tool_ok as f64) / (tool_total as f64)
                    } else {
                        1.0
                    };
                    let details = serde_json::json!({
                        "router_stats": {
                            "category": category_for_log(router_decision.category),
                            "strategy": strategy_for_log(router_decision.strategy),
                        },
                        "tool_success_rate": tool_success_rate,
                    });
                    let _ = log_event(
                        &app_config_dir,
                        &session_id,
                        None,
                        "agent",
                        "orchestrator",
                        "session_summary",
                        Some("complete"),
                        LogMetrics {
                            tokens_in: Some(totals.tokens_in),
                            tokens_out: Some(totals.tokens_out),
                            cost_usd: Some(totals.cost_usd),
                            ..Default::default()
                        },
                        details,
                    );
                }
            }
        }
    }

    Ok(action)
}

/// List recent sessions from appdata/logs/*.jsonl (excludes telemetry.jsonl).
#[tauri::command]
fn list_sessions(app: AppHandle) -> Result<Vec<oxcer::telemetry_viewer::SessionSummary>, String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    oxcer::telemetry_viewer::list_sessions_from_dir(&app_config_dir)
}

/// Load one session's log events from logs/{session_id}.jsonl.
#[tauri::command]
fn load_session_log(app: AppHandle, session_id: String) -> Result<Vec<LogEvent>, String> {
    let app_config_dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    oxcer::telemetry_viewer::load_session_log_from_dir(&app_config_dir, &session_id)
}

/// Returns the Tauri context for the app.
fn app_context() -> tauri::Context<tauri::Wry> {
    #[cfg(all(test, feature = "test"))]
    {
        tauri::test::mock_context(tauri::test::noop_assets())
    }

    #[cfg(not(all(test, feature = "test")))]
    {
        tauri::generate_context!()
    }
}

fn main() {
    let context = app_context();
    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(PendingApprovalsStore::new())
        .manage(AgentSessionStore::new())
        .manage(SessionLlmMetricsStore::new())
        .manage(SessionCostAlertedStore::new())
        .manage(Mutex::new(AppSettings::default()))
        .setup(|app| {
            let handle = app.handle();
            let app_config_dir = handle.path().app_config_dir().expect("app_config_dir");
            std::fs::create_dir_all(&app_config_dir)
                .expect("failed to create app config directory");
            let loaded = settings_load(&app_config_dir);
            if let Some(state) = handle.try_state::<Mutex<AppSettings>>() {
                *state.lock().expect("settings lock") = loaded;
            }

            // Sprint 9: Load plugins, merge into catalog, init policy with plugin rules
            let plugins_dir = app_config_dir.join("plugins");
            let descriptors =
                match load_plugins_from_dir_with_telemetry(&plugins_dir, &app_config_dir, "system")
                {
                    Ok(d) => d,
                    Err(e) => {
                        eprintln!("[plugins] load failed: {}", e);
                        vec![]
                    }
                };
            let plugin_rules = plugin_rules_from_descriptors(&descriptors);
            let base_yaml = include_str!("../../../../oxcer-core/policies/default.yaml");
            let base_config = load_from_yaml(base_yaml.as_bytes());
            let merged_config = merge_rules(base_config, plugin_rules);
            let _ = init_policy_with_config(merged_config);

            let mut catalog = shell::default_catalog();
            let shell_specs = shell_plugins_to_command_specs(&descriptors);
            catalog.merge_plugin_commands(shell_specs);
            handle.manage(Arc::new(catalog));

            let capability_registry = build_capability_registry(&descriptors);
            handle.manage(std::sync::Mutex::new(capability_registry));

            // App menu: Oxcer (Quit); in debug builds also View -> Toggle Developer Tools
            let quit = PredefinedMenuItem::quit(handle, None)?;
            let app_sub = SubmenuBuilder::new(handle, "Oxcer").item(&quit).build()?;

            #[cfg(debug_assertions)]
            {
                let devtools_item = MenuItem::with_id(
                    handle,
                    "devtools",
                    "Toggle Developer Tools",
                    true,
                    Some("CmdOrCtrl+Shift+I"),
                )?;
                let view_sub = SubmenuBuilder::new(handle, "View")
                    .item(&devtools_item)
                    .build()?;
                let menu = MenuBuilder::new(handle)
                    .items(&[&app_sub, &view_sub])
                    .build()?;
                app.set_menu(menu)?;
                app.on_menu_event(move |app_handle, event| {
                    if event.id().0.as_str() == "devtools" {
                        if let Some(w) = app_handle.get_webview_window("main") {
                            w.open_devtools();
                        }
                    }
                });
                // Open devtools on startup in debug builds
                if let Some(w) = app.get_webview_window("main") {
                    w.open_devtools();
                }
            }

            #[cfg(not(debug_assertions))]
            {
                let menu = MenuBuilder::new(handle).items(&[&app_sub]).build()?;
                app.set_menu(menu)?;
            }

            Ok(())
        })
        // ONLY invoke commands for FS/Shell — no bypass. Agent Orchestrator
        // must use these; direct fs::/shell:: calls are not exposed.
        .register_uri_scheme_protocol("oxcer", |_ctx, request| {
            let path = request.uri().path();
            let (status, body, content_type) = match path {
                "/main" | "main" | "/" | "" => (
                    StatusCode::OK,
                    DASHBOARD_HTML.as_bytes().to_vec(),
                    "text/html; charset=utf-8",
                ),
                _ => (StatusCode::NOT_FOUND, b"Not Found".to_vec(), "text/plain"),
            };
            Response::builder()
                .status(status)
                .header(CONTENT_TYPE, content_type)
                .body(body)
                .unwrap()
        })
        .invoke_handler(tauri::generate_handler![
            cmd_fs_list_dir,
            cmd_fs_read_file,
            cmd_fs_write_file,
            cmd_fs_delete,
            cmd_fs_rename,
            cmd_fs_move,
            cmd_shell_run,
            cmd_approve_and_execute,
            get_setup_status,
            start_model_download,
            complete_setup,
            get_external_api_warning,
            cmd_settings_get,
            cmd_settings_save,
            cmd_dialog_open_directory,
            cmd_workspace_add,
            cmd_workspace_remove,
            cmd_models_list,
            cmd_llm_invoke,
            cmd_scrub_payload_for_llm,
            cmd_agent_step,
            list_sessions,
            load_session_log,
            get_config,
            get_command_visibility,
            get_effective_fs_policy,
            cmd_plugin_capabilities,
        ])
        .run(context)
        .expect("error while running Oxcer Tauri application");
}
