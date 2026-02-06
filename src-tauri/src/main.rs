#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

#[tauri::command]
fn list_dir(path: String) -> Result<Vec<String>, String> {
    let path = PathBuf::from(path);
    let entries = fs::read_dir(&path)
        .map_err(|e| format!("Failed to read dir {path:?}: {e}"))?;

    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("Dir entry error: {e}"))?;
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }

    Ok(names)
}

#[tauri::command]
fn read_file(path: String) -> Result<String, String> {
    let path = PathBuf::from(path);
    let mut file = fs::File::open(&path)
        .map_err(|e| format!("Failed to open file {path:?}: {e}"))?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .map_err(|e| format!("Failed to read file {path:?}: {e}"))?;
    Ok(contents)
}

#[tauri::command]
fn write_file(path: String, contents: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    let mut file = fs::File::create(&path)
        .map_err(|e| format!("Failed to create file {path:?}: {e}"))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| format!("Failed to write file {path:?}: {e}"))?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_dir, read_file, write_file])
        .run(tauri::generate_context!())
        .expect("error while running Oxcer Tauri application");
}

