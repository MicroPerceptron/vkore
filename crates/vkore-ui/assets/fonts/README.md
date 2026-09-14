# Bundled font provenance

`JetBrainsMono-Medium.ttf` is the unmodified official JetBrains Mono v2.304
static Medium face (weight 500):

- Source: https://github.com/JetBrains/JetBrainsMono/blob/cd5227bd1f61dff3bbd6c814ceaf7ffd95e947d9/fonts/ttf/JetBrainsMono-Medium.ttf
- SHA-256: `31c92d01a8a08528b718a43addf0ad3df0af2ca4b7b3290a452f70f358e14d3d`
- License: `OFL-JetBrainsMono.txt` in this directory.

It replaces the previous Medium asset whose OS/2 weight was 436. The UI asks
for weight 500; the mismatch made shaping select an unregistered macOS system
font. Keep the asset's declared weight aligned with the UI request. Registration
and rendering must continue to use the actual font bytes, not a substituted
system font or a relaxed resource check.

The other bundled faces and their license files are unchanged.
