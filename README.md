# LittleBritain Universal Modding Tool

V0.4A
- This version implements the ability to load SCN files in the modding tool. (scene/level files)
- View all static meshes and terrain in the level files.
- Added fly controls in the 3D viewport. (see controls below)
- Added a fly speed slider.
- Implemented multi-material support on a single mesh -> needed to display the terrain correctly.
- When loading a level, you will see 'scene nodes' at the bottom of the mod tool. These are bundles of objects you can click on to quickly move toward them.
- Added a toggle for 'markers' of yet to be placed / not yet understood objects. (mostly skeletal meshes & scripts)
- Added a toggle for 'Shadow Blobs / Decals'.
- Small tweaks to the shader that is used when disabling textures.
- Changed the 'Scenes' category name to 'Levels'.
- Small layout tweaks.

V0.3
- Added light/dark mode toggle - by default the tool launches in dark mode
- Added ffmpeg libraries to support reading the bik files
- Added video playback and audio in a new bik viewer window
- Included playback slider, audio slider and zoom functionality in the bik viewer window
- Overall stability improvements

V0.2
- Cross reference: models -> textures, textures -> models with clickable buttons
- Display missing textures
- Toggles in the 3d model viewer now include: faces, textures, wireframe, culling
- Improved the overall custom shader in the viewport
- You can now zoom inside models without the camera bumping into the 3d model
- Added a ground plane

V0.1
- categorization of the files
- reading dds files
- reading all naked audio files
- reading the bnk files
- reading geo files fully -> both rigged and static geo's
