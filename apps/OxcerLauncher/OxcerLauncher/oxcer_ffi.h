#ifndef oxcer_ffi_h
#define oxcer_ffi_h

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/// All functions take and return UTF-8 JSON strings. Caller must call oxcer_string_free on every returned non-null pointer.

/// Input: {} or { "app_config_dir": "/path" }. Output: { "workspaces": [ { "id", "name", "root_path" }, ... ] }.
const char* oxcer_list_workspaces(const char* json_in);

/// Input: {} or { "app_config_dir": "/path" }. Output: JSON array of session summaries.
const char* oxcer_list_sessions(const char* json_in);

/// Input: { "session_id": "..." } and optional "app_config_dir". Output: JSON array of LogEvent.
const char* oxcer_load_session_log(const char* json_in);

/// Input: { "task_description": "...", "workspace_id"?, "workspace_root"?, "context"? }. Output: { "ok": true, "answer": "...", "error": null } or { "ok": false, "error": "..." }.
const char* oxcer_agent_request(const char* json_in);

/// Free a string returned by any oxcer_* function. Safe to call with NULL.
void oxcer_string_free(char* ptr);

#ifdef __cplusplus
}
#endif

#endif /* oxcer_ffi_h */
