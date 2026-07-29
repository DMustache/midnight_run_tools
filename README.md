# Midnight Run Tools

Windows tools for Dota 2 Dark Carnival / Midnight Run minigames.

The project is organized as a Rust library plus one binary per tool. Shared code handles Win32 screen capture/input, the overlay, and the common minigame launch flow.

## Requirements

- Windows
- Dota 2 running in a visible window
- Rust MSVC toolchain, for example `stable-x86_64-pc-windows-msvc`
- Visual Studio Build Tools with the C++ workload for release builds

Automation Attack also requires Tesseract OCR installed. Install instructions are in the official Tesseract README: <https://github.com/tesseract-ocr/tesseract#installing-tesseract>. If Tesseract is missing, the tool shows the message in the overlay, waits 10 seconds, then exits.

## Build

```powershell
rustup default stable-x86_64-pc-windows-msvc
cargo check --all-targets
cargo build --release
```

Build one tool:

```powershell
cargo build --release --bin boot_breaker
cargo build --release --bin automation_attack
```

Release executables are written to:

```text
target/release/
```

## Common runtime behavior

All tools use the same launcher:

1. Open Dota 2.
2. Open the Dark Carnival minigame panel.
3. Start the executable.
4. The tool detects the minigame frame.
5. The tool detects and clicks the PLAY button.
6. The tool re-detects the gameplay area.
7. The overlay shows status, script FPS, CPU load, and tool-specific messages.

Stop any tool with:

```text
Ctrl + Shift + Q
```

No command-line arguments are currently supported. Configuration is in each tool's `Args::default()` implementation.

Debug builds show extra overlay drawings. These drawings are visible to the user but are excluded from screen capture so the detector does not see them.

## Binaries

### boot_breaker

Runs the Boot Breaker minigame helper.

Run from source:

```powershell
cargo run --release --bin boot_breaker
```

Run built executable:

```powershell
.\target\release\boot_breaker.exe
```

What it does:

- detects and clicks PLAY;
- detects the inner Boot Breaker play zone;
- tracks the flying boot;
- detects the cart;
- moves the cart under the currently detected boot position;
- rethrows when the boot settles on the cart;
- writes diagnostics to `boot_breaker_debug.tsv`.

Debug build:

```powershell
cargo run --bin boot_breaker
```

Debug overlay colors:

- white rectangle: captured gameplay area;
- gray rectangle: boot search area;
- cyan/orange rectangles: active boot search ROIs;
- green box/cross: current boot detection;
- yellow cross/vector: tracked boot state;
- magenta line: cart detection Y;
- red line: boot bottom cutoff;
- magenta cross: raw cart detection;
- blue cross/vector: tracked cart state;
- yellow vertical line: current target X.

### automation_attack

Runs the Automation Attack minigame helper.

Run from source:

```powershell
cargo run --release --bin automation_attack
```

Run built executable:

```powershell
.\target\release\automation_attack.exe
```

What it does:

- checks for Tesseract OCR before starting the game;
- detects and clicks PLAY only if Tesseract is available;
- reads the active word prompt;
- uses embedded seed templates from `automation_attack_seed_templates.bin`;
- trains/extends the in-memory template database while running;
- types detected words into the game;
- writes diagnostics to `automation_attack_debug.tsv`.

Tesseract lookup order:

1. `C:\Program Files\Tesseract-OCR\tesseract.exe`
2. `tesseract` from `PATH`

If Tesseract is not installed, install it and ensure `tesseract --version` works from PowerShell.

## Logs

Default log files:

```text
boot_breaker_debug.tsv
automation_attack_debug.tsv
```

These are tab-separated files intended for detector/control debugging.
