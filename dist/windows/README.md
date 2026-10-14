# Windows icon
`octodock.ico` contains 16, 24, 32, 48, 64, 128, and 256 pixel PNG frames
drawn by `scripts/draw-octopus-logo.py`. Each frame is rendered at its own size
with transparent corners; frames up to 32 pixels use a full-bleed tile and a
larger octopus so the eyes stay distinct.

`apps/octodock/build.rs` compiles `octodock.rc` into the Windows executable for
both debug and release builds. Resource ID 1 is required by GPUI's Windows
icon loader. No adjacent image file is needed at runtime; the installer
(`octodock.iss`) uses the same `octodock.ico` for its own window and the uninstall
entry.
