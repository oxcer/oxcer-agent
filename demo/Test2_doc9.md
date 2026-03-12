# FsMove and Cross-Directory Operations

## What FsMove Does

`FsMove` relocates a single file from one path to another, potentially across workspace roots. It uses Foundation's `FileManager.moveItem(atPath:toPath:)` under the hood. On the same volume, this is a rename operation at the filesystem level and is effectively instantaneous regardless of file size. Across volumes, Foundation copies then deletes, which takes longer and is not atomic.

## Cross-Workspace Design

Unlike the other filesystem tools, `FsMove` carries two workspace roots: `workspace_root` (source) and `dest_workspace_root` (destination). This is necessary for the demo Workflow 3, which moves files from `~/Downloads` to `~/Desktop/Test_folder` — two different well-known directories that each have their own workspace context. The orchestrator encodes both roots explicitly so the Swift executor can construct both absolute paths without ambiguity.

## Destination Path Construction

The destination path is `dest_workspace_root + "/" + dest_rel_path`. In the move workflow, `dest_rel_path` is `dest_rel_dir + "/" + filename` (e.g. `"Test_folder/Test2_doc1.md"`). This means the destination folder must already exist before any `FsMove` step runs — which is why the orchestrator inserts exactly one `FsCreateDir` step as the first action after `FsListDir`, before any of the `FsMove` steps.

## Per-File Approval

Each `FsMove` step triggers its own approval request. For a 20-file move workflow this means 20 separate approvals, which is intentional: the user should see exactly what is being moved and where. A future version may add a "batch approve" option for multi-file operations, but for v0.1.0 the per-step approval model is preserved for safety.

## Error Recovery

If one `FsMove` step fails (because the source file does not exist or the destination is not writable), the orchestrator immediately returns an error and halts. Files that were already moved in earlier steps are not rolled back. This matches the behaviour of a standard shell `mv` command: there is no transactional filesystem undo. The approval prompt for each file gives the user the option to stop the sequence before it starts.
