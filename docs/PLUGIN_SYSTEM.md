# Plugin System (Sprint 9)

## Overview

Oxcer supports a YAML-defined plugin system for extending capabilities without recompiling. All plugin execution goes through:

- **Command Router** → **Security Policy Engine** → Approval (where applicable) → tool
- No plugin bypasses data sensitivity filters (Sprint 7).

---

## git_status end-to-end walkthrough

The `plugins/shell/git_status.yaml` plugin serves as the **canonical E2E demo**. It exercises the entire stack.

### 1. Load

On app startup, the loader scans `plugins/` recursively for `.yaml` files. Each file is parsed into a `PluginDescriptor`. The loader validates:

- `id` is non-empty and unique
- `type` is `shell` | `fs_indexer` | `agent_tool`
- Shell plugins have non-empty `binary_path` and `template`
- `security` maps to valid policy types

If validation fails, the plugin is skipped with a logged error. Valid descriptors are kept.

### 2. Catalog merge

Shell (and fs_indexer with binary_path) plugins are converted to `CommandSpec` via `shell_plugins_to_command_specs()`. Each spec is merged into the `CommandCatalog` via `merge_plugin_commands()`. Plugin IDs override built-in commands on collision. The catalog uses a `HashMap` for O(1) lookup by id.

Result: `shell.git_status` is available as a command and can be invoked like any built-in tool.

### 3. Security Policy Engine

For each plugin, `plugin_rules_from_descriptors()` generates a policy rule from the `security` block:

- `tool_type` → policy match
- `operations` → for shell, mapped to `exec` (invocations always use Exec)
- `dangerous` → if true, rule action is `RequireApproval`; otherwise `Allow`
- `require_approval` → explicit override
- `risk_level` → policy metadata

Plugin rules are prepended to the base policy. For `shell.git_status` (operations: read, dangerous: false), the rule allows execution without approval.

### 4. Semantic Router / tool hints

Shell and agent_tool plugins with `schema.category_hint` are registered in the `CapabilityRegistry`. The registry is indexed by `category_hint` and `tags` for efficient lookup.

When routing a task, the router checks whether the task text contains any category_hint. `git_status` has `category_hint: "git"`, so a task like "show me the git status" produces `tool_hints: ["shell.git_status"]`. The frontend or orchestrator can use these hints to prefer those commands.

### 5. Telemetry

During plugin load, `load_plugins_from_dir_with_telemetry()` emits:

- `plugin_start` — at the start of load (details: plugins_dir)
- `plugin_end` — at the end (details: loaded_count or error)

These appear in the session trace (`logs/{session_id}.jsonl`) and rolling telemetry (`logs/telemetry.jsonl`).

---

## Plugin types

| Type         | Description                           | Binary required | Registers to                |
|--------------|---------------------------------------|-----------------|-----------------------------|
| `shell`      | Shell command wrapper                 | Yes             | Command catalog             |
| `fs_indexer` | FS indexer/scanner (e.g. git repo)    | Optional        | Command catalog (if binary) |
| `agent_tool` | High-level agent capability           | No              | Capability registry         |

## YAML spec format

Plugins live under `plugins/` (relative to app config dir, e.g. `~/Library/Application Support/Oxcer/plugins/` on macOS).

### Shell plugin

```yaml
id: "shell.git_status"
type: "shell"
binary_path: "/usr/bin/git"
template: ["-C", "{{workspace}}", "status", "--short"]
schema:
  description: "Show git status in the current workspace"
  args:
    - name: "workspace_id"
      type: "string"
      required: true
security:
  tool_type: [shell]
  operations: [read]
  dangerous: false
```

### FS indexer plugin

```yaml
id: "fs.git_indexer"
type: "fs_indexer"
binary_path: "oxcer-fs-git-indexer"
schema:
  description: "Scan git repo and build a code index"
  args: []
security:
  tool_type: [fs]
  operations: [read]
  dangerous: false
```

### Agent tool plugin

```yaml
id: "agent.deploy_tool"
type: "agent_tool"
schema:
  description: "Deploy current project with predefined steps"
  category_hint: "deploy"
security:
  tool_type: [shell, network]
  operations: [exec]
  dangerous: true
```

## Security block

- **tool_type**: `shell`, `fs`, `agent`, `network`, `web`, `other`
- **operations**: `read`, `write`, `exec`, `delete`, `rename`, `move`, `chmod`
- **dangerous**: If `true`, plugin requires approval by default.
- **risk_level**: `low`, `medium`, `high` (for policy metadata)
- **require_approval**: Explicit override (overrides `dangerous` default when set)

## Validation

- `id` must be unique.
- `type` must be `shell` | `fs_indexer` | `agent_tool`.
- `security` must map to valid policy types.
- Shell plugins require non-empty `binary_path` and `template`.
- Invalid plugins are skipped with logged errors.

## Capability registry

Agent tools and shell/fs_indexer plugins with `category_hint` register in the **Capability Registry**. The registry is indexed by `category_hint` and `tags` for O(1) lookups:

- `for_category(category)` — capabilities whose category_hint matches
- `for_tag(tag)` — capabilities that have the tag
- `matching_ids_for_task(task)` — ids whose category_hint or tags appear in the task text

The Semantic Router uses `matching_ids_for_task` to populate `tool_hints` when the task matches.

Query capabilities via `cmd_plugin_capabilities`.

## Telemetry

Plugin load emits:

- `plugin_start` — at start of load (component: `plugin`, action: `plugin_start`)
- `plugin_end` — at finish (component: `plugin`, action: `plugin_end`, decision: `ok` | `error`)

---

## Manual QA checklist: git_status E2E

1. **Setup**
   - Copy `plugins/shell/git_status.yaml` to `~/Library/Application Support/Oxcer/plugins/` (macOS).
   - Ensure the `plugins` directory exists; create if needed.

2. **Launch**
   - Run `pnpm tauri dev` (or the packaged app).
   - The app loads plugins at startup. Check logs for `plugin_start` and `plugin_end`.

3. **Catalog**
   - Invoke `cmd_shell_run` with `command_id: "shell.git_status"` and valid `workspace_root` + `params: { workspace_id: "..." }`.
   - Expect success (stdout contains git status or empty for non-repo).

4. **Policy**
   - As agent: same invocation should be allowed (git_status has dangerous: false).
   - As agent: a plugin with `dangerous: true` would require approval before execution.

5. **Router tool hints**
   - Call `cmd_agent_step` with task "show me the git status".
   - Inspect the session's `router_output.tool_hints` — should include `shell.git_status` when capabilities are present.

6. **Telemetry**
   - Inspect `~/Library/Application Support/Oxcer/logs/telemetry.jsonl`.
   - Find events with `component: "plugin"` and `action: "plugin_start"` / `plugin_end`.
