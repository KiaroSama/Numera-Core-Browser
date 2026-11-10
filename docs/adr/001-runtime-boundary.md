# ADR 001 — shared Rust runtime, owned Chromium milestone

Accepted for the initial development milestone, 2026-11-10.

Use the Orbita product boundary, independently implemented from licensed public sources: Numera Browser owns profile records, scheduling, vault data and user choices; Numera Core Browser owns the selected browser and its native resources. No second profile dashboard, cloud sync service or required manager installation is introduced.

The first browser milestone is an owned Chromium/Blink/V8 source build with retained upstream browser chrome. Source and depot_tools commits and patch hashes are explicit. The current branding patch is the first source-queue entry; it does not provide fingerprint control, automation compatibility or a completed browser release. Windows/Linux x86-64 and macOS ARM64 are baseline targets, with Intel macOS tracked separately.

Rust is the primary first-party control-plane language. Upstream Chromium remains largely C/C++; the project does not claim a majority-Rust renderer. Generated schemas, license notices and upstream-derived patches are counted separately from authored source. Native bridges will be added only with tested needs and licenses.

Gecko and WebKit remain explicit future families after the user's Orbita direction. One shared runtime can select different engines; it cannot make one renderer behave identically to all three. No engine substitution or genuine Safari/iOS claim is allowed.

No engine is currently installed, so spawn paths fail closed. Process ownership, native GUI interaction, profile isolation and real automation must be implemented and tested before enabling session launch. CLI and future GUI must use the same runtime handlers for all nonvisual operations.
