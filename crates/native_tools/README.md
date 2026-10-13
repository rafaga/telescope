# native_tools

Operating-system specific code for [Telescope](../../README.md), behind one
interface per task.

| Module | Purpose |
|--------|---------|
| `dialog` | Native open file / folder dialogs: `IFileOpenDialog` on Windows, `NSOpenPanel` on macOS, and the XDG desktop portal (`ashpd`) on Linux. A dialog can be owned by the application's window and start in a chosen folder, and its result is a `DialogResult` (chosen, cancelled or failed). |
| `overlay` | The overlay drawn over the game: `OverlayWindow` sets the native window flags (topmost, click-through, out of the task switcher; on macOS also visible over a full screen app; on Linux through X11, also XWayland, not native Wayland) `OverlayPlacement` keeps the overlay's position as a distance from the nearest corner of the game window (so it can be dragged, survives the game moving and, once calibrated, scales with the window's height on any screen), and `FullScreenState` detects exclusive full screen on Windows (where no overlay can show), and `GameWindowTracker` finds the EVE window's position (Windows, X11 and macOS; macOS needs no permission: CoreGraphics' window list and `NSWorkspace`). Nothing is injected into the game. |
| `permissions` | `AccessibilityPermission`: checks and asks for the macOS Accessibility permission (global shortcuts, other windows' positions). Always "granted" on the other systems. |
| `zbus` (Linux only) | Machine identification: the DMI UUID, D-Bus (`org.freedesktop.hostname1`) and the machine-id files, tried in an order that does not depend on running as root. |
| `lib.rs` | The per-OS `get_*_unique_id` functions. |

The machine identifier is what `webb` derives the encryption key of the player
database from, so it has to stay the same: on Linux `get_stable_machine_id` reads
the machine-id files first, because every user can read them.

The dialogs and the identification code are platform code and are tested on the
platform they belong to: the `zbus` tests run on Linux only. CI runs the tests on
Linux, Windows and macOS.

```sh
cargo test -p native_tools
```
