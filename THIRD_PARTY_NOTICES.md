# Third-party notices

Window caption geometry, vector glyph construction, native hit testing and system-menu
handling in `src/window-chrome.ts`, `src/window-chrome.css` and
`src-tauri/src/window_chrome.rs` are adapted from ContactSolutions:
https://github.com/william-pyle/ContactSolutions
commit `6f97202b4bceadbd6e468e345bd8f9622f7861ad`, `src/chrome.rs` and
`src/window_chrome.rs`. The palette is from its `src/theme.rs`.

Copyright (c) 2026 William Pyle. MIT License, reproduced in
`licenses/ContactSolutions-LICENSE.txt` and included in the installer payload.

Retained source notice: ContactSolutions' look (palette, type scale, widget metrics,
layout) follows MailCraft's design (MIT OR Apache-2.0), shared with MessageCraft.
No donor fonts, contact store, messaging bridge, or application data are bundled.

The native-frame fallback was informed by read-only inspection of MessageSolutions
`src/window_chrome.rs` at `b1796db730c897f7ecd203b2e897f4eae7d01a3f`.
No MessageSolutions source was copied.
