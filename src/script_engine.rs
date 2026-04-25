use std::fs;
use std::path::Path;

pub struct ScriptEngine {
    pub logs: Vec<String>,
    pending_load: Option<String>,
    pending_save: Option<String>,
}

impl ScriptEngine {
    pub fn new() -> Self {
        Self { 
            logs: Vec::new(),
            pending_load: None,
            pending_save: None,
        }
    }

    pub fn init(&mut self) {}

    pub fn run_script(&mut self, path: &Path) -> Result<(), String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read script: {}", e))?;

        self.logs.push(format!("=== Running: {:?} ===", path.file_name().unwrap_or_default()));

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Err(e) = self.execute_line(line) {
                self.logs.push(format!("Error: {}", e));
            }
        }

        self.logs.push("=== Done ===".to_string());
        Ok(())
    }

    fn extract_quoted(&self, line: &str) -> Option<String> {
        let line = line.trim();
        if let Some(start) = line.find('"') {
            if let Some(end) = line[start+1..].find('"') {
                return Some(line[start+1..start+1+end].to_string());
            }
        }
        None
    }

    fn execute_line(&mut self, line: &str) -> Result<(), String> {
        let line = line.trim();
        if line.is_empty() { return Ok(()); }

        let upper = line.to_uppercase();

        if upper.starts_with("LOAD_SYMBOLS") {
            if let Some(path) = self.extract_quoted(line) {
                self.logs.push(format!("[load_symbols] {}", path));
            } else {
                return Err("Usage: LOAD_SYMBOLS \"path\"".into());
            }
            Ok(())
        }
        else if upper.starts_with("LOAD_SCN") {
            if let Some(path) = self.extract_quoted(line) {
                self.logs.push(format!("[load_scn] Queued: {}", path));
                self.pending_load = Some(path);
            } else {
                return Err("Usage: LOAD_SCN \"path\"".into());
            }
            Ok(())
        }
        else if upper.starts_with("SAVE_SCN") {
            if let Some(path) = self.extract_quoted(line) {
                self.logs.push(format!("[save_scn] Queued: {}", path));
                self.pending_save = Some(path);
            } else {
                return Err("Usage: SAVE_SCN \"path\"".into());
            }
            Ok(())
        }
        else if upper.starts_with("UPDATE_NODE_TRANSFORM") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 18 { return Err("Usage: UPDATE_NODE_TRANSFORM idx m00..m33".into()); }
            let idx = parts[1];
            self.logs.push(format!("[update_node_transform] Queued for node {}", idx));
            Ok(())
        }
        else if upper.starts_with("ADD_NODE") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 6 { return Err("Usage: ADD_NODE parent kind name archetype flags".into()); }
            self.logs.push(format!("[add_node] Queued: parent={} kind={} name={}", parts[1], parts[2], parts[3]));
            Ok(())
        }
        else if upper.starts_with("REMOVE_NODE") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 { return Err("Usage: REMOVE_NODE idx".into()); }
            self.logs.push(format!("[remove_node] Queued: node {}", parts[1]));
            Ok(())
        }
        else if upper.starts_with("SET_NODE_POSITION") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 { return Err("Usage: SET_NODE_POSITION idx x y z".into()); }
            self.logs.push(format!("[set_position] Queued: node {} at ({},{},{})", parts[1], parts[2], parts[3], parts[4]));
            Ok(())
        }
        else if upper.starts_with("SET_NODE_ROTATION") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 { return Err("Usage: SET_NODE_ROTATION idx x y z".into()); }
            self.logs.push(format!("[set_rotation] Queued: node {} to ({},{},{})", parts[1], parts[2], parts[3], parts[4]));
            Ok(())
        }
        else if upper.starts_with("SET_NODE_SCALE") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 { return Err("Usage: SET_NODE_SCALE idx x y z".into()); }
            self.logs.push(format!("[set_scale] Queued: node {} to ({},{},{})", parts[1], parts[2], parts[3], parts[4]));
            Ok(())
        }
        else if upper.starts_with("HIDE_NODE") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 { return Err("Usage: HIDE_NODE idx".into()); }
            self.logs.push(format!("[hide_node] Queued: {}", parts[1]));
            Ok(())
        }
        else if upper.starts_with("SHOW_NODE") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 { return Err("Usage: SHOW_NODE idx".into()); }
            self.logs.push(format!("[show_node] Queued: {}", parts[1]));
            Ok(())
        }
        else if upper.starts_with("LIST_NODES") {
            self.logs.push("[list_nodes] Queued".into());
            Ok(())
        }
        else if upper.starts_with("PRINT") {
            let msg = &line[6..].trim();
            self.logs.push(format!("[print] {}", msg));
            Ok(())
        }
        else if upper.starts_with("LOG") {
            if let Some(msg) = self.extract_quoted(line) {
                self.logs.push(format!("[log] {}", msg));
            } else {
                let msg = &line[4..].trim();
                self.logs.push(format!("[log] {}", msg));
            }
            Ok(())
        }
        else {
            self.logs.push(format!("[unknown] {}", line.split_whitespace().next().unwrap_or("")));
            Ok(())
        }
    }

    pub fn get_logs(&self) -> &Vec<String> {
        &self.logs
    }

    pub fn get_pending_load(&self) -> Option<String> {
        self.pending_load.clone()
    }

    pub fn get_pending_save(&self) -> Option<String> {
        self.pending_save.clone()
    }

    pub fn clear_logs(&mut self) {
        self.logs.clear();
    }
}

impl Default for ScriptEngine {
    fn default() -> Self {
        Self::new()
    }
}