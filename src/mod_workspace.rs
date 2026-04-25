use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct ModManifest {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub entry_script: String,
    pub language: String,
}

impl Default for ModManifest {
    fn default() -> Self {
        Self {
            name: "MyLuaMod".to_owned(),
            version: "0.1.0".to_owned(),
            author: "Unknown".to_owned(),
            description: "A Little Britain Lua mod.".to_owned(),
            entry_script: "scripts/main.lua".to_owned(),
            language: "lua".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModPackage {
    pub manifest: ModManifest,
    pub path: PathBuf,
    pub manifest_path: PathBuf,
    pub scripts: Vec<PathBuf>,
    pub assets: Vec<PathBuf>,
    pub patches: Vec<PathBuf>,
}

pub fn mods_dir(game_root: &Path) -> PathBuf {
    game_root.join("Mods")
}

pub fn scan_mods(game_root: &Path) -> Result<Vec<ModPackage>> {
    let root = mods_dir(game_root);
    fs::create_dir_all(&root).with_context(|| format!("Creating {}", root.display()))?;

    let mut mods = Vec::new();
    for entry in fs::read_dir(&root).with_context(|| format!("Reading {}", root.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Ok(package) = load_mod(&path) {
                mods.push(package);
            }
        }
    }

    mods.sort_by_key(|package| package.manifest.name.to_ascii_lowercase());
    Ok(mods)
}

pub fn create_lua_mod(game_root: &Path, requested_name: &str) -> Result<ModPackage> {
    let mods_root = mods_dir(game_root);
    fs::create_dir_all(&mods_root).with_context(|| format!("Creating {}", mods_root.display()))?;

    let safe_name = sanitize_mod_name(requested_name);
    let mod_dir = unique_mod_dir(&mods_root, &safe_name);
    let scripts_dir = mod_dir.join("scripts");
    let assets_dir = mod_dir.join("assets");
    let patches_dir = mod_dir.join("patches");

    fs::create_dir_all(&scripts_dir)
        .with_context(|| format!("Creating {}", scripts_dir.display()))?;
    fs::create_dir_all(&assets_dir)
        .with_context(|| format!("Creating {}", assets_dir.display()))?;
    fs::create_dir_all(&patches_dir)
        .with_context(|| format!("Creating {}", patches_dir.display()))?;

    let manifest = ModManifest {
        name: safe_name.clone(),
        description: format!("{} scripting mod.", safe_name),
        ..ModManifest::default()
    };

    fs::write(mod_dir.join("mod.json"), manifest_to_json(&manifest))
        .with_context(|| format!("Writing {}", mod_dir.join("mod.json").display()))?;
    fs::write(
        scripts_dir.join("main.lua"),
        lua_template(&safe_name, &manifest.version),
    )
    .with_context(|| format!("Writing {}", scripts_dir.join("main.lua").display()))?;
    fs::write(mod_dir.join("README.txt"), readme_template())
        .with_context(|| format!("Writing {}", mod_dir.join("README.txt").display()))?;

    load_mod(&mod_dir)
}

pub fn create_lua_script(mod_dir: &Path, requested_name: &str) -> Result<PathBuf> {
    let scripts_dir = mod_dir.join("scripts");
    fs::create_dir_all(&scripts_dir)
        .with_context(|| format!("Creating {}", scripts_dir.display()))?;

    let mut safe_name = sanitize_mod_name(requested_name);
    if !safe_name.to_ascii_lowercase().ends_with(".lua") {
        safe_name.push_str(".lua");
    }

    let mut script_path = scripts_dir.join(&safe_name);
    if script_path.exists() {
        let stem = safe_name.trim_end_matches(".lua");
        for index in 2..1000 {
            let candidate = scripts_dir.join(format!("{}_{}.lua", stem, index));
            if !candidate.exists() {
                script_path = candidate;
                break;
            }
        }
    }

    let script_name = script_path
        .file_stem()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "script".to_owned());

    fs::write(&script_path, lua_script_template(&script_name))
        .with_context(|| format!("Writing {}", script_path.display()))?;

    Ok(script_path)
}

pub fn read_text_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("Reading {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn write_text_file(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("Writing {}", path.display()))
}

fn load_mod(path: &Path) -> Result<ModPackage> {
    let manifest_path = path.join("mod.json");
    let manifest = if manifest_path.is_file() {
        parse_manifest(&read_text_file(&manifest_path)?)
    } else {
        let mut fallback = ModManifest::default();
        fallback.name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "UnnamedMod".to_owned());
        fallback
    };

    Ok(ModPackage {
        manifest,
        manifest_path,
        scripts: collect_files_with_extensions(&path.join("scripts"), &["lua"]),
        assets: collect_files(&path.join("assets")),
        patches: collect_files_with_extensions(&path.join("patches"), &["patch"]),
        path: path.to_path_buf(),
    })
}

fn collect_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files_recursive(dir, None, &mut out);
    out.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    out
}

fn collect_files_with_extensions(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_files_recursive(dir, Some(extensions), &mut out);
    out.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    out
}

fn collect_files_recursive(dir: &Path, extensions: Option<&[&str]>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(&path, extensions, out);
            continue;
        }

        let matches_extension = extensions
            .map(|extensions| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| {
                        extensions
                            .iter()
                            .any(|wanted| ext.eq_ignore_ascii_case(wanted))
                    })
                    .unwrap_or(false)
            })
            .unwrap_or(true);

        if matches_extension {
            out.push(path);
        }
    }
}

fn parse_manifest(text: &str) -> ModManifest {
    let mut manifest = ModManifest::default();

    if let Some(value) = json_string_value(text, "name") {
        manifest.name = value;
    }
    if let Some(value) = json_string_value(text, "version") {
        manifest.version = value;
    }
    if let Some(value) = json_string_value(text, "author") {
        manifest.author = value;
    }
    if let Some(value) = json_string_value(text, "description") {
        manifest.description = value;
    }
    if let Some(value) = json_string_value(text, "entry_script") {
        manifest.entry_script = value;
    }
    if let Some(value) = json_string_value(text, "language") {
        manifest.language = value;
    }

    manifest
}

fn json_string_value(text: &str, key: &str) -> Option<String> {
    let key_needle = format!("\"{}\"", key);
    let key_start = text.find(&key_needle)?;
    let after_key = &text[key_start + key_needle.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();

    if !after_colon.starts_with('"') {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for ch in after_colon[1..].chars() {
        if escaped {
            let decoded = match ch {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            };
            value.push(decoded);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some(value);
        } else {
            value.push(ch);
        }
    }

    None
}

fn manifest_to_json(manifest: &ModManifest) -> String {
    format!(
        concat!(
            "{{\n",
            "  \"name\": \"{}\",\n",
            "  \"version\": \"{}\",\n",
            "  \"author\": \"{}\",\n",
            "  \"description\": \"{}\",\n",
            "  \"language\": \"{}\",\n",
            "  \"entry_script\": \"{}\",\n",
            "  \"enabled\": \"true\"\n",
            "}}\n"
        ),
        escape_json(&manifest.name),
        escape_json(&manifest.version),
        escape_json(&manifest.author),
        escape_json(&manifest.description),
        escape_json(&manifest.language),
        escape_json(&manifest.entry_script),
    )
}

fn escape_json(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| match ch {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            other => vec![other],
        })
        .collect()
}

fn sanitize_mod_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_was_separator = false;

    for ch in name.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_separator = false;
        } else if !last_was_separator && !out.is_empty() {
            out.push('_');
            last_was_separator = true;
        }
    }

    while out.ends_with('_') {
        out.pop();
    }

    if out.is_empty() {
        "MyLuaMod".to_owned()
    } else {
        out
    }
}

fn unique_mod_dir(mods_root: &Path, base_name: &str) -> PathBuf {
    let mut candidate = mods_root.join(base_name);
    if !candidate.exists() {
        return candidate;
    }

    for index in 2..1000 {
        candidate = mods_root.join(format!("{}_{}", base_name, index));
        if !candidate.exists() {
            return candidate;
        }
    }

    mods_root.join(format!("{}_{}", base_name, uuidish_suffix()))
}

fn uuidish_suffix() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "copy".to_owned())
}

fn lua_template(mod_name: &str, version: &str) -> String {
    format!(
        r#"-- Little Britain Lua Mod
-- Name: {mod_name}
-- Version: {version}
--
-- Runtime note:
-- The mod tool can create and edit this script now.
-- The game will only execute it after the Bink proxy loads modloader.dll
-- and the modloader embeds/hosts Lua.

local mod = {{
    name = "{mod_name}",
    version = "{version}",
}}

local function log(message)
    if lb and lb.log then
        lb.log(message)
    else
        print(message)
    end
end

function on_mod_loaded()
    log("Loaded " .. mod.name .. " v" .. mod.version)
end

function on_level_loaded(level_name)
    log("Level loaded: " .. tostring(level_name))
end

function on_update(delta_time)
    -- Called every frame once runtime scripting is wired.
end

on_mod_loaded()
"#,
        mod_name = mod_name,
        version = version,
    )
}

fn lua_script_template(script_name: &str) -> String {
    format!(
        r#"-- Little Britain Lua Script
-- Script: {script_name}

local M = {{}}

function M.on_level_loaded(level_name)
    if lb and lb.log then
        lb.log("{script_name}: level loaded " .. tostring(level_name))
    end
end

function M.on_update(delta_time)
    -- Add per-frame behavior here once runtime scripting is wired.
end

return M
"#,
        script_name = script_name,
    )
}

fn readme_template() -> &'static str {
    "Little Britain Lua Mod\n\nPut runtime Lua files in scripts/.\nPut replacement assets in assets/ using Data-relative paths later.\nPut binary patch definitions in patches/ later.\n\nThis package format is intentionally simple so it can be loaded by the mod tool and the future in-game modloader.\n"
}
