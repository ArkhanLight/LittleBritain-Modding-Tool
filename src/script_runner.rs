use std::path::{Path, PathBuf};
use std::{fs, io::{self, Read}};

use anyhow::{Result, Context};
use scn::{load_scn, ScnFile};

// Minimal script runner for the mod tool
// Script language (very tiny):
// - LOAD_SCN "path/to.scn"
// - LOAD_SYMBOLS "path/to/COMPLETE_symbol_dump.h"  (optional, just logs)
// - UPDATE_NODE_TRANSFORM idx v0 v1 ... v15
// - ADD_NODE parent_id "name" "archetype" v0..v15 flags
// - REMOVE_NODE index
// - SAVE_SCN "path/to.scn"
// - LOG "message"

fn parse_f32s(words: &[&str], count: usize) -> Option<[f32; 16]> {
    if words.len() < count { return None; }
    let mut arr = [0f32; 16];
    for i in 0..16 {
        arr[i] = words[i].parse::<f32>().ok()?;
    }
    Some(arr)
}

fn read_quoted(path_str: &str) -> String {
    // extract text inside quotes if present
    let mut s = path_str.to_string();
    if let Some(start) = s.find('"') {
        if let Some(end) = s[start+1..].find('"') {
            return s[start+1..start+1+end].to_string();
        }
    }
    s
}

fn main() -> Result<()> {
    // Very small CLI: first arg is script file path
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("Usage: script_runner <path_to_script>");
        return Ok(());
    }
    let script_path = PathBuf::from(&args[1]);
    let mut script = String::new();
    fs::File::open(&script_path).with_context(|| format!("Opening script {}", script_path.display()))?.read_to_string(&mut script)?;

    let mut scn_opt: Option<ScnFile> = None;
    let mut last_log = String::new();
    for raw in script.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        // Quick split by spaces, preserving quoted args with read_quoted
        // We'll crude-split on spaces while keeping quoted strings intact
        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut in_quote = false;
        for ch in line.chars() {
            if ch == '"' {
                in_quote = !in_quote;
                cur.push(ch);
            } else if ch == ' ' && !in_quote {
                if !cur.is_empty() { parts.push(cur.clone()); cur.clear(); }
            } else {
                cur.push(ch);
            }
        }
        if !cur.is_empty() { parts.push(cur); }
        if parts.is_empty() { continue; }
        match parts[0].as_str() {
            "LOAD_SCN" => {
                let path = read_quoted(&line);
                let p = Path::new(&path);
                scn_opt = Some(load_scn(p)?);
                last_log = format!("Loaded SCN: {}", path);
                println!("{}", last_log);
            }
            "LOAD_SYMBOLS" => {
                let path = read_quoted(&line);
                // Best-effort: just log
                last_log = format!("Loaded symbol dump: {}", path);
                println!("{}", last_log);
            }
            "UPDATE_NODE_TRANSFORM" => {
                if parts.len() >= 17 {
                    let idx = parts[1].parse::<usize>().unwrap_or(0);
                    if let Some(transform) = parse_f32s(&parts[2..], 16) {
                        if let Some(ref mut scn) = scn_opt {
                            scn.update_node_transform(idx, transform);
                            last_log = format!("Updated node {} transform", idx);
                            println!("{}", last_log);
                        }
                    }
                }
            }
            "ADD_NODE" => {
                // ADD_NODE parent_id "name" "archetype" 16 numbers flags
                if parts.len() >= 20 {
                    let parent = parts[1].parse::<usize>().unwrap_or(0);
                    let name = read_quoted(&line); // crude
                    let archetype = parts[3].trim_matches('"').to_string();
                    if let Some(transform) = parse_f32s(&parts[4..], 16) {
                        let flags = parts[20].parse::<u16>().unwrap_or(0);
                        if let Some(ref mut scn) = scn_opt {
                            scn.add_node(name, archetype, transform, flags);
                            last_log = format!("Added node under {}", parent);
                            println!("{}", last_log);
                        }
                    }
                }
            }
            "REMOVE_NODE" => {
                if parts.len() >= 2 {
                    let idx = parts[1].parse::<usize>().unwrap_or(0);
                    if let Some(ref mut scn) = scn_opt {
                        scn.remove_node(idx);
                        last_log = format!("Removed node {}", idx);
                        println!("{}", last_log);
                    }
                }
            }
            "SAVE_SCN" => {
                let path = read_quoted(&line);
                if let Some(ref mut scn) = scn_opt {
                    scn.save_scn(&Path::new(&path))?;
                    last_log = format!("Saved SCN to {}", path);
                    println!("{}", last_log);
                }
            }
            "LOG" => {
                // LOG "text" inside line
                let msg = read_quoted(&line);
                println!("LOG: {}", msg);
                last_log = msg;
            }
            _ => {
                // Unknown command
            }
        }
    }
    Ok(())
}
