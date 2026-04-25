# 🎮 Little Britain Modding Platform

**A complete reverse-engineering and modding toolkit for Little Britain (2001)**

![Status](https://img.shields.io/badge/status-Active-brightgreen)
![C++](https://img.shields.io/badge/C++-17-blue)
![Rust](https://img.shields.io/badge/Rust-2024-orange)
![License](https://img.shields.io/badge/License-MIT-yellow)

---

## 📦 Features

### 🔧 Mod Tool (Rust + egui)
- **Asset Viewer** - View/edit DDS textures, GEO models, ANM animations, BIK videos
- **Scene Editor** - Inspect and modify .scn level files
- **Audio Bank Explorer** - Browse BNK audio banks
- **Built-in Script Runner** - Automate level modifications

### 💉 Mod Loader (C++ DLL)
- **Game Injection** - Loads mods when game starts
- **Lua Scripting** - Mods can call game functions
- **Binary Patching** - Hook into game code safely

### 🔬 Decompiled Code
- **90+ Game Classes** extracted from the original EXE
- **470+ Function symbols** mapped and documented
- **Source reconstruction** templates

---

## 🎯 What Can Be Modded?

| Category | Examples |
|----------|----------|
| 🏃 **New Levels** | Add custom Vicky skate levels |
| 👾 **New Characters** | Barbara, Marjorie, Meera, Paul, Pat, Tanya |
| 🎮 **Game Mechanics** | Modify DaffydGame, FrogGame, CPukeGame |
| 🖼️ **Textures** | Swap or create new DDS textures |
| 📐 **Models** | Import custom GEO geometry |
| 🎬 **Animations** | New ANM animation sequences |
| 🔊 **Audio** | Custom BNK sound banks |

---

## 🗂️ Project Structure

```
Mod Tool/
├── little_britain_mod_tool/     # Main Rust application
│   ├── src/
│   │   ├── app.rs              # Main UI (egui)
│   │   ├── scn.rs               # Scene file parser/saver
│   │   ├── geo.rs               # GEO model handler
│   │   ├── anm.rs               # Animation handler
│   │   ├── script_engine.rs     # Built-in scripting
│   │   └── mod_manager.rs       # Mod creation/management
│   └── scripts/                # Example scripts
├── modloader/                   # C++ DLL injector
│   └── modloader.cpp            # Mod loader source
└── mods/                       # Mod storage
    └── ExampleMod/             # Example mod structure
```

---

## 🚀 Quick Start

### 1. Run the Mod Tool
```bash
cd "Mod Tool\little_britain_mod_tool"
cargo run
```

### 2. Open Game Data
- Click **Open game folder**
- Select the **Data** folder from Little Britain installation

### 3. Explore Assets
- Browse textures (.dds), models (.geo), scenes (.scn)
- Use the 3D viewer to inspect levels
- Preview animations and videos

### 4. Create a Mod
```
mods/YourMod/
├── mod.json        # Mod metadata
├── scripts/        # Lua scripts
├── patches/        # Binary patches
└── assets/         # New textures/models
```

---

## 📜 Game Classes (90+ Extracted)

```
Game Levels:        Characters:       Engine:
├─ VickyGame       ├─ Character      ├─ App
├─ DaffydGame      ├─ Player         ├─ Display
├─ AnneGame        ├─ Barbara        ├─ Scene
├─ FrogGame        ├─ Marjorie       ├─ Skeleton
├─ CPukeGame       ├─ Meera          ├─ Animation
├─ cDivingGame     ├─ Paul           ├─ Texture
├─ cfootballgame   ├─ Pat            ├─ LightManager
└─ cpirategame     └─ Tanya          └─ NodeFactory
```

---

## 🔗 Extracted Symbols

### RTTI Class Hierarchy
```
Character (base class)
├── Player
├── PukableCharacter
└── Ghost

GameBaseClass (base class)
├── VickyGame
├── DaffydGame
├── AnneGame
├── FrogGame
├── CPukeGame
└── cDivingGame
```

### Key Engine Functions
- `App::Init()` - Application startup
- `Scene::Load()` - Level loading
- `Display::Init()` - Direct3D initialization
- `Skeleton::CreateBone()` - Character rig building
- `Animation::Play()` - Animation playback

---

## 🛠️ Development

### Requirements
- **Rust** (latest stable)
- **Visual Studio 2022** (for C++ DLL)
- **Ghidra** (for further reverse engineering)

### Build ModLoader.dll
```batch
cd src/modloader
cl /LD /EHsc modloader.cpp /Fe:modloader.dll
```

### Run Mod Tool
```batch
cargo run
```

---

## 📚 Documentation

- [Modding Platform Guide](MODDING_PLATFORM.md)
- [Ghidra Quick Start](ghidra_scripts/GHIDRA_QUICKSTART.md)
- [Mod Loader README](README_MODS.md)

---

## 🎨 Screenshots

*(Placeholder for mod tool screenshots)*

```
┌─────────────────────────────────────┐
│  Little Britain Mod Tool            │
├──────────────┬──────────────────────┤
│ File Tree    │ Inspector            │
│              │                      │
│ ▼ Data       │ Path: ...\vicky.dds  │
│   ▼ Vicky    │ Size: 512x512        │
│     textures │ Format: DXT5        │
│     models   │                      │
│     scenes   │ [Scripts]           │
│   ▼ Anne     │ Script: _________   │
│   ▼ Daffyd   │ [Run Script]         │
│              │                      │
├──────────────┴──────────────────────┤
│  Preview: 3D View / Texture View   │
└─────────────────────────────────────┘
```

---

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Submit a pull request

---

## ⚠️ Disclaimer

This project is for educational purposes and reverse-engineering research. 

---

**Made with ❤️ for the Little Britain gaming community**