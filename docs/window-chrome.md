# Integrated window chrome

## Current Explorer direction

The owner approved the supplied Explorer layout on 2026-11-19. The current
caption uses a 40 logical pixel tab strip, 46 by 32 caption controls and a 16 pixel
blue FileSolutions icon in a 20 pixel system-menu target. `src/explorer.css`
applies Explorer neutral colors and integrated folder tabs. The vector glyphs
and Windows hit-testing foundation remain adapted from the donor below.
Native normal/maximized light/dark views were observed at 125% DPI. System menu,
titlebar dragging, double-click, restore, corner resizing, minimize and
close-to-tray passed isolated checks. The local backup service survives closing
the window. See `explorer-verification.json` for exact binaries and remaining
external drag/drop, mixed-DPI, snap, picker and clean-VM acceptance gaps.

## Historical ContactSolutions caption adaptation

The following describes the preceding caption revision; its dimensions and
palette are superseded by the approved Explorer direction above. Donor provenance
and license obligations remain applicable.

The FileSolutions caption is adapted from ContactSolutions commit
`6f97202b4bceadbd6e468e345bd8f9622f7861ad`, not drawn with font characters.
Donor source is `src/chrome.rs` (`TITLE_BAR`, `title_bar`, `window_buttons`),
`src/window_chrome.rs` (hit testing and system menu), and `src/theme.rs` (colors).
The MIT license and retained look attribution are included in
`THIRD_PARTY_NOTICES.md` and `licenses/ContactSolutions-LICENSE.txt`.

The caption is 48 logical pixels high. Each caption button is 42 pixels wide;
its stroke is 1 pixel and its glyph bounds are 10 by 10. Restore adds the donor's
back top/right edges offset by (+2, -2). The 24-pixel app tile starts at x=12 and
has a 32-pixel menu hit target. The caption separator uses the source border at
60% opacity. Hover, focus and pressed states share the source hover wash;
Close uses #C42B3B. There is no separate invented pressed color.

The application keeps Rust/Tauri/WebView2. SVG paths replace egui painter calls;
an inset SVG centerline reproduces egui's inside rectangle stroke. CSS dimensions
are translated to physical pixels for the adapted Win32 subclass. Edge/corner
hit tests take priority, disappear while maximized, and exclude icon/controls
from caption dragging. Signed screen coordinates support negative monitor origins.
Tao retains window styles, work-area sizing, DPI changes and minimum size handling.
Tauri's undecorated shadow mode is disabled so its existing native child resize
handler covers all four edges and corners. With shadows enabled it covers only
the top edge; actual right/bottom pointer resize failed in the fixture. This
platform adaptation removes the external shadow, not the integrated controls.

Wry 0.57.1 enables WebView2 Settings9 nonclient regions. CSS `app-region: drag`
on the titlebar and `no-drag` on buttons route child-window input directly to
Windows. The initial IPC forwarding adaptation failed an actual pointer drag
while the donor passed the same gesture; it was removed. The native region
support supplies dragging, double click and drag from maximized without an IPC
race. See [Microsoft Settings9 documentation](https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2settings9).
Window resize/state events
update the maximize/restore glyph and accessible name even for OS-driven changes.
The app icon, blank-caption right-click and Alt+Space open the real system menu,
anchored below the icon with Restore/Size/Move/Maximize enabled appropriately.

Purposeful adaptations: the blue folder tile stays #0F68F0 in both themes as
requested; ContactSolutions changes its tile accent in dark mode. FileSolutions
keeps its backup service running when Close or Alt+F4 hides the window; Quit in
the tray exits. The title text replaces ContactSolutions' contact-specific search
and quick actions. A visible keyboard focus outline and white focused Close glyph
improve keyboard readability. MessageSolutions' native-frame fallback informed
the error path; no MessageSolutions code or messaging components were copied.
If subclass setup fails, Windows decorations are restored, custom controls are
hidden and the interface reports the fallback.

Automated geometry, IPC and color tests are not physical-input acceptance.
Isolated native desktop captures were compared against ContactSolutions at
120 DPI in both themes, normal and maximized. Actual pointer dragging,
double-click maximize, Restore, right-edge and corner resizing, and system-menu
interaction were observed. Snap, mixed-monitor DPI transitions, every physical
resize direction and assistive-technology acceptance remain open. The native
helper cannot drag outside a source window, so external drag/drop is unverified.
See `chrome-verification.json` for exact artifact coverage, failures, screenshots
and remaining gaps. Earlier approximate-caption installers remain historical.
