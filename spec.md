# Minimal Wayland Window Manager

## Vision

Most of the time I switch between full-screen applications: several IntelliJ windows, several Ghostty terminals, Chrome and more. Switching between them quickly, including between windows of the same application, is the core of the workflow. Sometimes I snap a few applications to halves or corners of the screen to see them side by side.

Later, related applications should be groupable into a layer that comes into view all at once. For example, a set of terminals running htop, ctop, k9s and nvtop, shown together as one monitoring view.

The rest of this document was generated as a starting point and is not a complete statement of this vision. Where the two disagree, the vision wins. Items that are ~~struck through~~ are implemented. Work beyond this document is tracked in GitHub issues.

## 1. Goal

Build a minimal, keyboard-driven Wayland compositor/window manager for Linux using **Rust + Smithay**.

The project is intentionally small. It is not intended to become a desktop environment or a general-purpose window manager.

The desired workflow is:

* ~~Applications initially open fullscreen.~~
* ~~`Super` opens an application launcher.~~
* ~~The application launcher is provided by **Fuzzel**, running as a separate Wayland application.~~
* ~~`Alt+Tab` switches between applications.~~
* ~~`Super+Left/Right` snaps the focused application to the left/right half.~~
* ~~`Super+Up/Down` snaps the focused application to the top/bottom half.~~
* ~~Combining horizontal and vertical directions places the focused application in a quarter of the screen.~~

The compositor should contain as little UI functionality as possible.

---

# 2. Technology

## Language

Use:

* Rust stable
* Cargo
* Current stable Smithay release

Do not use obsolete Smithay versions merely to follow old examples.

Before implementation, inspect the current Smithay documentation, examples, and APIs.

## Platform

Primary target:

* Fedora Linux
* Wayland
* DRM/KMS
* Linux input devices

The compositor should eventually run directly as a Wayland session.

Do not initially target:

* macOS
* Windows
* X11 as the primary backend

The compositor contains no XWayland or other X11 code. X11-only applications run through [xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite), a separate program that runs a rootless Xwayland and shows each X11 window as a normal xdg-shell window. When it is installed, the compositor starts it on a free X display once its Wayland socket is listening, stops it on exit, and gives launched applications that display as `DISPLAY`. It needs only `xdg_wm_base` and `wp_viewporter` from the compositor.

---

# 3. Core Design Principle

Keep the compositor small.

Use Smithay for as much infrastructure as possible.

Do not implement:

* Wayland protocol handling from scratch
* DRM handling from scratch
* input handling from scratch
* rendering from scratch
* EGL/OpenGL setup from scratch
* damage tracking from scratch
* application discovery
* fuzzy searching
* launcher UI

The compositor should primarily implement **window-management policy**.

Conceptually:

```text
                    Smithay
                       │
        ┌──────────────┼──────────────┐
        │              │              │
     Wayland         Input         Rendering
     protocol
        │              │              │
        └──────────────┼──────────────┘
                       │
                ┌──────▼──────┐
                │    MyWM     │
                │             │
                │ Window list │
                │ Focus       │
                │ Layout      │
                │ Keybinds    │
                └─────────────┘
```

---

# 4. Window Management

Maintain a collection of managed application windows.

Each managed window should have, as appropriate:

* unique internal identifier
* Wayland surface
* mapped/unmapped state
* focus state
* current layout
* geometry
* associated output

The compositor must maintain an explicit currently focused window.

When a new top-level application appears:

1. ~~Register/manage it.~~
2. ~~Add it to the managed-window collection.~~
3. ~~Give it keyboard focus.~~
4. ~~Make it fullscreen.~~
5. ~~Render it on the appropriate output.~~

---

# ~~5. Initial Window State~~

Every newly managed top-level application should initially occupy the entire usable output.

No window decorations are required.

For example:

```text
┌──────────────────────────────────────┐
│                                      │
│                                      │
│             Application              │
│                                      │
│                                      │
└──────────────────────────────────────┘
```

When a new application is launched from Fuzzel, it should therefore automatically appear fullscreen.

---

# ~~6. Layouts~~

Define a clear layout abstraction.

For example:

```rust
enum Layout {
    Fullscreen,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
```

The exact implementation may differ, but layout calculations must be isolated from low-level input and Wayland code.

---

# ~~7. Layout Geometry~~

For an output with:

```text
width
height
```

use these layouts.

### ~~Fullscreen~~

```text
x = 0
y = 0
width = output_width
height = output_height
```

### ~~Left~~

```text
x = 0
y = 0
width = output_width / 2
height = output_height
```

### ~~Right~~

```text
x = output_width / 2
y = 0
width = output_width - output_width / 2
height = output_height
```

### ~~Top~~

```text
x = 0
y = 0
width = output_width
height = output_height / 2
```

### ~~Bottom~~

```text
x = 0
y = output_height / 2
width = output_width
height = output_height - output_height / 2
```

### ~~Top-left~~

```text
x = 0
y = 0
width = output_width / 2
height = output_height / 2
```

### ~~Top-right~~

```text
x = output_width / 2
y = 0
width = output_width - output_width / 2
height = output_height / 2
```

### ~~Bottom-left~~

```text
x = 0
y = output_height / 2
width = output_width / 2
height = output_height - output_height / 2
```

### ~~Bottom-right~~

```text
x = output_width / 2
y = output_height / 2
width = output_width - output_width / 2
height = output_height - output_height / 2
```

Use integer-safe calculations so odd output dimensions do not produce gaps or overlaps.

---

# ~~8. Keyboard Shortcuts~~

The initial keybindings are:

```text
Alt+Tab        Focus previous application (MRU order)

Super          Launch application launcher

Super+Left     Left half
Super+Right    Right half
Super+Up       Top half
Super+Down     Bottom half
```

Quarter-screen layouts:

```text
Super+Left + Up       Top-left
Super+Right + Up      Top-right
Super+Left + Down     Bottom-left
Super+Right + Down    Bottom-right
```

The exact interaction model for the directional combinations is up to the implementation, but it must be reliable.

For example, holding Super and pressing:

```text
Left → Up
```

should result in the top-left layout.

Likewise:

```text
Right → Down
```

should result in bottom-right.

---

# ~~9. Super Key Handling~~

Standalone `Super` and `Super+key` must be distinguished.

Desired behavior:

```text
Super              → launch Fuzzel

Super+Left         → snap left
Super+Right        → snap right
Super+Up           → snap top
Super+Down         → snap bottom
```

The compositor must not launch Fuzzel when Super is used as part of a window-management shortcut.

Avoid introducing a noticeable delay merely to determine whether Super is standalone.

Use Smithay's keyboard/input infrastructure.

Do not implement a separate raw keyboard subsystem.

---

# ~~10. Application Launcher~~

The compositor must **not** implement its own application launcher UI.

Use **Fuzzel** as an external application launcher.

Fuzzel is responsible for:

* application discovery
* `.desktop` file parsing
* fuzzy searching
* application names
* application icons
* search UI
* keyboard navigation
* launching applications

The compositor's only responsibility is to start Fuzzel when standalone `Super` is pressed.

Conceptually:

```text
Super
  │
  ▼
MyWM
  │
  │ spawn
  ▼
Fuzzel
  │
  ├── search installed applications
  ├── select application
  └── launch application
          │
          ▼
       Wayland
          │
          ▼
        MyWM
          │
          ▼
    fullscreen window
```

Do not implement:

* `.desktop` parsing
* application indexing
* fuzzy matching
* icons
* application categories
* favorites
* recent applications
* file search
* web search
* calculator
* command execution
* plugins

The launcher should remain completely independent of the compositor.

Fuzzel should be configurable independently through its normal configuration mechanism.

The compositor should simply invoke the configured Fuzzel executable.

---

# 11. Focus Handling

Implement application switching using:

```text
Alt+Tab
```

Switching uses most-recently-used (MRU) order, like Alt+Tab on other desktops.

Maintain a focus history: the most recently focused window first.

Behavior:

* ~~Pressing `Alt+Tab` once focuses the previously focused window, so a quick `Alt+Tab` toggles between the two most recent windows.~~
* ~~While `Alt` stays held, each further `Tab` press steps one window deeper into the MRU list.~~
* ~~Wrap around at the end of the list.~~
* ~~While `Alt` is held, draw a horizontal row of live window thumbnails in MRU order in the middle of the screen, each with the application's icon in its lower right corner and the application's name under it, with the selected one highlighted and its title shown below the row. Stepping only moves the highlight; windows are not raised until `Alt` is released.~~
* ~~Show the list only after a short delay (150 ms) or at the second `Tab`, so a quick `Alt+Tab` does not flash it.~~
* ~~`Shift+Tab` steps backwards. While the list is open, `Up`/`Left` and `Down`/`Right` also move the highlight, and `Escape` closes it and keeps the current window.~~
* ~~The MRU list is not reordered while `Alt` is held; otherwise stepping would keep bouncing between the same two windows.~~
* ~~When `Alt` is released, the selected window moves to the front of the MRU list.~~
* ~~Do not change the selected window's layout.~~
* ~~If only one window exists, do nothing.~~

Any other focus change (a new window appearing, a pointer click) also moves that window to the front of the MRU list.

When a window is destroyed:

* ~~Remove it from the managed-window collection and the MRU list.~~
* ~~If it was focused, focus the next window in MRU order.~~

Focus order should be deterministic given the same sequence of events.

The MRU logic should be plain data-structure code, independent of Smithay, so it can be unit tested.

Do not initially implement:

* `Alt+Shift+Tab`
* workspaces
* virtual desktops
* window groups

---

# ~~12. Multiple Windows~~

Windows should be independently positioned.

Only the focused window changes geometry when a layout command is issued.

Other windows retain their existing layouts.

Overlapping windows are acceptable.

For example:

```text
Window A → left half
Window B → right half
Window C → fullscreen
```

This is valid.

Do not implement automatic tiling.

---

# 13. Multiple Monitors

Multi-monitor support is a **nice-to-have**, not a requirement for the first usable release.

The first version targets a single output. If more than one output is connected, using only one of them is acceptable.

Keep the architecture from precluding multiple outputs later:

* ~~Layout calculations take the output's geometry (including its position) as input rather than assuming a single global screen.~~
* ~~A window records which output it belongs to, even if there is only one.~~
* ~~Avoid global "the screen size" state.~~

Do not let multi-monitor concerns delay or complicate the single-output implementation.

If multi-monitor support is added later, each output should have:

* geometry
* dimensions
* associated windows

A window's layout is then calculated relative to its current output.

Do not implement:

* workspaces per monitor
* monitor-specific configuration UI
* automatic window migration policies

---

# 14. Wayland Protocols

Use Smithay's current protocol implementations.

Investigate and implement the minimum required protocols for normal graphical Wayland applications, including the appropriate Smithay support for:

* ~~`wl_compositor`~~
* ~~`wl_shm`~~
* ~~`wl_seat`~~
* ~~keyboard~~
* ~~pointer~~
* ~~`xdg_wm_base`~~
* ~~`xdg_surface`~~
* ~~`xdg_toplevel`~~

The exact Smithay APIs should be determined from the current version.

Applications must be able to:

* ~~create surfaces~~
* ~~receive keyboard input~~
* ~~receive pointer input~~
* ~~resize~~
* ~~close~~
* ~~create/destroy surfaces~~

---

# 15. Mouse Input

Mouse input must work normally for applications.

Initially, the compositor does not need mouse-specific window-management commands.

Do not implement:

* mouse-driven resizing
* mouse-driven moving
* custom cursor themes
* mouse gestures

Keyboard-driven management is the primary interaction model.

---

# 16. Rendering

Use Smithay's renderer abstraction.

Do not implement a custom renderer.

Do not implement:

* animations
* shadows
* transparency effects
* blur
* custom window decorations
* compositor effects

The compositor should simply render client surfaces correctly.

---

# ~~17. Window Lifecycle~~

Correctly handle:

* ~~application creation~~
* ~~mapping~~
* ~~unmapping~~
* ~~destruction~~
* ~~focus~~
* ~~resize requests~~
* ~~client state changes~~
* ~~client termination~~

A crashed or badly behaved client must not crash the compositor.

When the focused application disappears, focus another available application.

---

# 18. Configuration

Configuration should initially be minimal.

Do not build a configuration GUI.

A future configuration file may look like:

```toml
[launcher]
command = "fuzzel"

[keybindings]
next_window = "Alt+Tab"
left = "Super+Left"
right = "Super+Right"
up = "Super+Up"
down = "Super+Down"
```

However, configuration support is **not required for the first working version**.

Hard-coded defaults are acceptable initially.

---

# 19. Logging

Use:

* ~~`tracing`~~
* ~~`tracing-subscriber`~~

Log useful events:

* ~~compositor startup~~
* ~~output creation/removal~~
* ~~window creation~~
* ~~window destruction~~
* ~~focus changes~~
* ~~layout changes~~
* ~~launcher invocation~~
* ~~client errors~~

Do not produce excessive logging from the render loop.

---

# 20. Error Handling

Use idiomatic Rust error handling.

Do not use `unwrap()`/`expect()` indiscriminately.

They are acceptable when an invariant is genuinely guaranteed and the reason is clear.

Startup failures should provide useful diagnostics for things such as:

* ~~DRM unavailable~~
* ~~permission problems~~
* ~~renderer initialization failure~~
* ~~Wayland socket failure~~
* ~~missing Fuzzel executable~~

Client failures should not bring down the compositor.

---

# ~~21. Testing~~

The geometry/layout logic must have unit tests independent of Smithay.

Test:

```text
1000 × 800

Fullscreen    0,0 1000×800
Left          0,0 500×800
Right         500,0 500×800
Top           0,0 1000×400
Bottom        0,400 1000×400

TopLeft       0,0 500×400
TopRight      500,0 500×400
BottomLeft    0,400 500×400
BottomRight   500,400 500×400
```

Also test odd dimensions:

```text
1921 × 1081
```

Verify that all layouts exactly cover the intended regions without gaps caused by integer rounding.

Test MRU focus logic separately: quick toggle between two windows, stepping deeper while Alt is held, commit on Alt release, and focus fallback when the focused window is destroyed.

---

# 22. Development Phases

Do not implement everything in one large change.

## ~~Phase 1 — Project~~

Create:

* ~~Cargo project~~
* ~~Smithay dependency~~
* ~~logging~~
* ~~module structure~~
* ~~basic executable~~

Verify:

```text
cargo check
```

works.

---

## ~~Phase 2 — Minimal Compositor~~

Implement the smallest viable Smithay compositor.

Get:

* ~~backend~~
* ~~renderer~~
* ~~output~~
* ~~event loop~~
* ~~Wayland socket~~

working.

The compositor should start successfully.

---

## ~~Phase 3 — First Application~~

Run a simple Wayland application such as:

```text
foot
```

Verify:

* ~~application starts~~
* ~~application appears~~
* ~~keyboard input works~~
* ~~mouse input works~~
* ~~application can close~~

---

## ~~Phase 4 — Window Management~~

Implement:

* ~~managed window collection~~
* ~~focus~~
* ~~fullscreen initial placement~~
* ~~window lifecycle~~

Verify that multiple applications can coexist.

---

## ~~Phase 5 — Layout Engine~~

Implement:

* ~~fullscreen~~
* ~~left~~
* ~~right~~
* ~~top~~
* ~~bottom~~
* ~~top-left~~
* ~~top-right~~
* ~~bottom-left~~
* ~~bottom-right~~

Add unit tests.

---

## ~~Phase 6 — Keyboard Management~~

Implement:

```text
Alt+Tab
Super+Left
Super+Right
Super+Up
Super+Down
```

Then implement quarter-screen combinations.

---

## ~~Phase 7 — Fuzzel~~

Install/use Fuzzel as an external launcher.

Implement:

```text
Super → spawn Fuzzel
```

Verify:

1. ~~Press Super.~~
2. ~~Fuzzel appears.~~
3. ~~Search for an application.~~
4. ~~Press Enter.~~
5. ~~Fuzzel launches the application.~~
6. ~~The new application appears fullscreen.~~
7. ~~Focus moves to the new application.~~

The compositor should contain no launcher UI code.

---

## Phase 8 — Multiple Outputs (optional)

Optional. Add multi-monitor handling only after the single-output compositor works well. Not required for the first usable release.

---

## ~~Phase 9 — Polish~~

Add:

* ~~robust error handling~~
* ~~useful logging~~
* ~~documentation~~
* ~~configuration~~
* ~~installation instructions~~
* ~~safe development workflow~~

---

# ~~23. Development Environment~~

The project must be easy to develop without replacing the user's normal desktop session.

The README must document a safe way to test the compositor.

Prefer a nested Wayland development workflow where practical.

Investigate the appropriate Smithay backend for running the compositor nested inside an existing Wayland session.

Do not instruct the user to replace GNOME or their existing display manager/session until the compositor is known to work.

---

# 24. Rust Guidelines

The developer is an experienced programmer but new to Rust.

Use idiomatic modern Rust without unnecessarily clever code.

Prefer:

* structs
* enums
* explicit ownership
* small functions
* meaningful types
* straightforward error propagation

Avoid unnecessary:

* macros
* unsafe code
* complex lifetime gymnastics
* excessive generics
* abstraction for abstraction's sake

If Smithay requires advanced Rust concepts, use them where necessary, but isolate complexity behind clear interfaces where practical.

Comments should explain **why**, particularly around Smithay's lifecycle, ownership, and rendering/input integration.

Do not dumb down the implementation.

---

# 25. Dependencies

Prefer a small and well-maintained dependency set.

Smithay should provide most compositor functionality.

Additional dependencies are acceptable when they provide clear value, especially for:

* logging
* configuration
* error handling
* serialization

Do not add a dependency simply to avoid writing a few straightforward lines of Rust.

---

# 26. Project Structure

A reasonable starting structure is:

```text
mywm/
├── Cargo.toml
├── README.md
├── src/
│   ├── main.rs
│   ├── compositor.rs
│   ├── state.rs
│   ├── input.rs
│   ├── keyboard.rs
│   ├── window.rs
│   ├── layout.rs
│   ├── output.rs
│   └── launcher.rs
└── tests/
```

The exact structure may change as the implementation develops.

Do not create modules that exist only to satisfy a predetermined architecture.

---

# 27. Agent Instructions

Before writing substantial code:

1. ~~Inspect the current Smithay documentation.~~
2. ~~Determine the current stable Smithay version.~~
3. ~~Inspect current Smithay compositor examples.~~
4. ~~Identify the recommended architecture for that version.~~
5. ~~Identify the appropriate Smithay backend for Fedora development.~~
6. ~~Determine how Smithay recommends handling:~~

   * DRM
   * rendering
   * outputs
   * input
   * xdg-shell
   * keyboard
   * pointer
   * client lifecycle
7. ~~Check how a nested development compositor can be run.~~
8. ~~Verify the current Fuzzel invocation and Wayland requirements.~~

Do not blindly copy old Smithay examples.

Smithay's API changes over time. Prefer current documentation and examples over old blog posts or outdated GitHub code.

Then implement incrementally.

After each major phase:

```text
cargo check
cargo test
```

Run additional formatting/linting where appropriate:

```text
cargo fmt
cargo clippy
```

Fix significant compiler warnings and errors before proceeding.

---

# 28. Scope Restrictions

Do not add functionality outside this specification unless explicitly requested.

In particular, do not spontaneously implement:

* panels
* notifications
* wallpaper
* system tray
* application menu
* workspaces
* virtual desktops
* XWayland inside the compositor (X11 applications use the external xwayland-satellite, see §2)
* window decorations
* animations
* compositor effects
* power controls
* network controls
* audio controls
* settings UI
* built-in terminal
* custom launcher
* application indexing
* desktop search

The one exception to power and audio controls: the brightness and volume keys (`XF86MonBrightnessUp`/`Down`, `XF86AudioRaiseVolume`/`LowerVolume`/`Mute`, `XF86AudioMicMute`) are bound to configurable outside commands, by default `brightnessctl` and `wpctl`. The compositor only runs the command; it contains no brightness or audio logic and shows no on-screen indicator.

Screen capture is supported so that outside tools work: the compositor implements the wlr-screencopy and ext-image-copy-capture protocols, which `grim` uses for screenshots and `xdg-desktop-portal-wlr` for screen sharing. The Print key runs a configurable command, by default `grim`. There is no capture interface in the compositor itself.

The purpose of the project is to have an extremely small and predictable Wayland environment.

---

# 29. Definition of Done

The first usable release is complete when:

1. ~~The compositor starts successfully on Fedora.~~
2. ~~A normal Wayland application can run inside it.~~
3. ~~Applications initially occupy the entire output.~~
4. ~~Multiple applications can run simultaneously.~~
5. ~~`Alt+Tab` switches focus between applications in most-recently-used order.~~
6. ~~`Super+Left` moves the focused application to the left half.~~
7. ~~`Super+Right` moves it to the right half.~~
8. ~~`Super+Up` moves it to the top half.~~
9. ~~`Super+Down` moves it to the bottom half.~~
10. ~~Direction combinations can place a window in all four quarters.~~
11. ~~Standalone `Super` launches Fuzzel.~~
12. ~~Fuzzel can launch applications normally.~~
13. ~~Newly launched applications automatically become focused and fullscreen.~~
14. ~~Applications can close without crashing the compositor.~~
15. ~~Layout calculations have unit tests.~~
16. ~~A single output works. Multi-monitor support is optional; the architecture must not preclude adding it later.~~
17. ~~The project has a README with Fedora dependencies, build instructions, safe testing instructions, and known limitations.~~
18. ~~No unnecessary desktop-environment functionality has been added.~~

The resulting system should feel like a deliberately minimal keyboard-driven Linux desktop rather than an attempt to recreate GNOME.

