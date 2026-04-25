use std::fs;
use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModInfo {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub game_version: String,
    pub dependencies: Vec<String>,
}

impl Default for ModInfo {
    fn default() -> Self {
        Self {
            name: "MyMod".to_string(),
            version: "1.0.0".to_string(),
            author: "Unknown".to_string(),
            description: "My awesome mod".to_string(),
            game_version: "1.0".to_string(),
            dependencies: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Mod {
    pub info: ModInfo,
    pub path: PathBuf,
    pub scripts: Vec<PathBuf>,
    pub patches: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
    pub enabled: bool,
}

impl Mod {
    pub fn create(&self, base_path: &Path) -> Result<(), String> {
        let mod_dir = base_path.join(&self.info.name);
        
        // Create directories
        fs::create_dir_all(&mod_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(mod_dir.join("scripts")).map_err(|e| e.to_string())?;
        fs::create_dir_all(mod_dir.join("patches")).map_err(|e| e.to_string())?;
        fs::create_dir_all(mod_dir.join("assets")).map_err(|e| e.to_string())?;
        
        // Write mod.json
        let json = serde_json::to_string_pretty(&self.info)
            .map_err(|e| e.to_string())?;
        fs::write(mod_dir.join("mod.json"), json).map_err(|e| e.to_string())?;
        
        // Create example script
        let example_script = r#"-- LittleBritain Mod Script
-- This script runs when the mod loads

log("Mod loaded: MyMod v1.0")

-- Example: Show a message when game starts
-- Mod_ShowMessage("Hello from MyMod!")

-- Example: Set player health
-- Mod_SetHealth(100)

-- Example: Unlock all levels
-- for i = 1, 10 do
--     Mod_UnlockLevel(i)
-- end

print("Mod initialization complete")
"#;
        fs::write(mod_dir.join("scripts").join("main.lua"), example_script)
            .map_err(|e| e.to_string())?;
        
        Ok(())
    }
    
    pub fn load_from_path(path: &Path) -> Result<Self, String> {
        let config_path = path.join("mod.json");
        if !config_path.exists() {
            return Err("mod.json not found".to_string());
        }
        
        let json = fs::read_to_string(&config_path)
            .map_err(|e| e.to_string())?;
        let info: ModInfo = serde_json::from_str(&json)
            .map_err(|e| e.to_string())?;
        
        let mut mod_obj = Mod {
            info,
            path: path.to_path_buf(),
            scripts: Vec::new(),
            patches: Vec::new(),
            assets: Vec::new(),
            enabled: false,
        };
        
        // Find scripts
        if let Ok(entries) = fs::read_dir(path.join("scripts")) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().map(|e| e == "lua" || e == "txt").unwrap_or(false) {
                    mod_obj.scripts.push(p);
                }
            }
        }
        
        // Find patches
        if let Ok(entries) = fs::read_dir(path.join("patches")) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().map(|e| e == "patch").unwrap_or(false) {
                    mod_obj.patches.push(p);
                }
            }
        }
        
        // Find assets
        if let Ok(entries) = fs::read_dir(path.join("assets")) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    mod_obj.assets.push(p);
                }
            }
        }
        
        Ok(mod_obj)
    }
}

pub struct ModManager {
    pub mods_dir: PathBuf,
    pub mods: Vec<Mod>,
    pub selected_mod: Option<usize>,
}

impl ModManager {
    pub fn new(mods_dir: PathBuf) -> Self {
        Self {
            mods_dir,
            mods: Vec::new(),
            selected_mod: None,
        }
    }
    
    pub fn load_mods(&mut self) -> Result<(), String> {
        self.mods.clear();
        
        if !self.mods_dir.exists() {
            fs::create_dir_all(&self.mods_dir).map_err(|e| e.to_string())?;
        }
        
        for entry in fs::read_dir(&self.mods_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            
            if path.is_dir() {
                match Mod::load_from_path(&path) {
                    Ok(m) => self.mods.push(m),
                    Err(e) => eprintln!("Failed to load mod at {:?}: {}", path, e),
                }
            }
        }
        
        Ok(())
    }
    
    pub fn create_mod(&mut self, info: ModInfo) -> Result<(), String> {
        let mod_obj = Mod {
            info,
            path: self.mods_dir.clone(),
            scripts: Vec::new(),
            patches: Vec::new(),
            assets: Vec::new(),
            enabled: false,
        };
        
        mod_obj.create(&self.mods_dir)?;
        self.load_mods()?;
        
        Ok(())
    }
    
    pub fn get_enabled_mods(&self) -> Vec<&Mod> {
        self.mods.iter().filter(|m| m.enabled).collect()
    }
}