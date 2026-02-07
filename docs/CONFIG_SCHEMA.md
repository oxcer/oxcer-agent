# config.json schema and UI mapping (Sprint 5)

Settings file: `config.json` under the app config directory (e.g. `~/Library/Application Support/Oxcer/config.json` on macOS, or `%APPDATA%/Oxcer/config.json` on Windows).

## Schema

```json
{
  "security": {
    "destructive_fs": { "enabled": false }
  },
  "workspaces": [
    { "id": "uuid", "name": "My Project", "root_path": "/path/to/dir" }
  ],
  "model": { "default_id": "gemini-2.5-flash" }
}
```

## UI control ↔ config field mapping (1:1)

| UI control | config.json field | type |
|------------|-------------------|------|
| "Allow destructive file operations" checkbox (Settings → Advanced) | `security.destructive_fs.enabled` | bool |
| Workspace list item id | `workspaces[].id` | string |
| Workspace list item display name | `workspaces[].name` | string |
| Workspace list item path | `workspaces[].root_path` | string |
| Default model dropdown (Settings → Basic) | `model.default_id` | string |

The default model selection is **stored only** in this sprint; Semantic Router / real model routing will be implemented in a later sprint.

## Backward compatibility

The loader also accepts legacy keys: `fs.destructive_operations_enabled` and top-level `default_model`. New saves use the schema above.
