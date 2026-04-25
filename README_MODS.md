# LittleBritain Mod Tool - Complete Package

## What's Included

### 1. ModLoader DLL (`src/modloader/`)
- C++ DLL that injects into the game
- Loads and manages mods at startup
- Provides API for mod scripts to call game functions
- Hook system for patching game code

**Build:**
```batch
cd src/modloader
g++ -shared -o modloader.dll modloader.cpp -O2
copy modloader.dll ..\..\dist\
```

### 2. Code Editor (`src/code_editor.rs`)
- Built-in script editor with syntax highlighting
- Support for Lua and C++
- File operations (New, Open, Save, Save As)
- Line numbers and status bar

### 3. Mod Manager (`src/mod_manager.rs`)
- Create, load, and manage mods
- Mod metadata (mod.json)
- Organize scripts, patches, assets

### 4. Example Mod (`mods/ExampleMod/`)
- Ready-to-use mod structure
- Example Lua script
- Metadata template

## Quick Start

1. **Build modloader.dll**
   ```batch
   cd "Mod Tool\little_britain_mod_tool\src\modloader"
   g++ -shared -o modloader.dll modloader.cpp
   ```

2. **Run the Mod Tool**
   ```batch
   cd "Mod Tool\little_britain_mod_tool"
   cargo run
   ```

3. **Create a mod**
   - Go to "Mods" tab
   - Click "New Mod"
   - Fill in details and click "Create"

4. **Write a script**
   - Use the built-in code editor
   - Save to `mods/YourMod/scripts/`

5. **Install modloader**
   - Copy `modloader.dll` to game folder
   - Rename original exe or create launcher

## Mod Structure

```
mods/
└── YourMod/
    ├── mod.json          # Mod metadata
    ├── scripts/          # Lua scripts
    │   └── main.lua      # Entry point
    ├── patches/          # Binary patches
    │   └── example.patch
    └── assets/           # New files
        └── textures/
```

## Mod API (for scripts)

```lua
-- Game functions
Mod_Log("Hello")              -- Log to console
Mod_ShowMessage("Hello!")    -- Show in-game message
Mod_SetHealth(100)           -- Set player health
Mod_UnlockLevel(3)          -- Unlock level
Mod_AddItem("coin")         -- Add item to inventory

-- Get game state
health = GetHealth()
current_level = GetCurrentLevel()
```

## Next Steps

1. **Decompile the game** - Need actual game function addresses
2. **Build modloader** - Get the DLL working
3. **Test mods** - Create and run a simple mod
4. **Add more features** - Asset importing, patch editor, etc.

---

For help, check the logs:
- `modloader.log` - DLL debug output
- ModTool console - Rust app debug output