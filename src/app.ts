import { invoke } from '@tauri-apps/api/core';

document.getElementById('test-fs')!.onclick = async () => {
  const files = await invoke('list_dir', { path: process.cwd() });
  document.getElementById('app')!.innerText = JSON.stringify(files, null, 2);
};
