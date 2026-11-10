# Numera Core Browser — Build and Implementation Instructions

**First obey the user's private Rules, hooks, and repository instructions. Discover and follow the actual Spec Kit workflow available in the environment; do not invent the contents of private rules or substitute a made-up command chain. Then implement this specification, test the result, and provide evidence.**

Document date: **2026-10-16**. This is an implementation handoff, not a statement that the repository, engines, or tests have already been created. The accompanying file `Numera-Browser-Core-Integration-Amendment-EN.md` is for the manager repository after a real Core integration release exists.

## 1. Locked decisions and project identity

Create the public repository **`KiaroSama/Numera-Core-Browser`**, human-readable product name **Numera Core Browser**, unless that exact repository already exists; then inspect and continue it without destructive reinitialization. Do not rename or replace `KiaroSama/Numera-Browser`.

The relationship is **Numera Browser = profile manager; Numera Core Browser = the actual browser/runtime launched by that manager**, analogous to the GoLogin/Orbita product boundary. Core must also be usable independently through its CLI and actual browser GUI. It must not become a second profile-management dashboard that duplicates the manager.

| Decision | Binding requirement |
|---|---|
| Architecture | Owned Rust runtime and owned, maintained upstream-derived engine builds; not a new HTML/CSS/JavaScript engine from scratch |
| Engines | Chromium/Blink/V8, Firefox/Gecko, and WebKit-derived support, with distinct packages and honest capability matrices |
| Main language | Rust is the primary and majority first-party implementation language; native upstream languages remain where technically necessary |
| Platforms | Windows, macOS, Linux; baseline Windows/Linux x86-64 and macOS Apple Silicon, plus macOS Intel and further architectures where validated |
| Interfaces | Separately installable CLI and browser GUI, optional engine packages, no required manager installation |
| Mobile | Android-like and iPhone/iOS-like desktop emulation; not native Android/iOS applications and not a promise of genuine mobile Safari |
| Automation | Real Playwright, Puppeteer, and Selenium interoperability through their appropriate supported protocols |
| Integration | A versioned local contract with Numera Browser; no cloud or cross-device sync project |
| CI/build budget | Free standard GitHub-hosted runners for this public repository only; no paid, self-hosted, workstation, VPS, or hidden external build service |
| Updates | Automated upstream tracking and safe client updates to approved, compatible Core/engine releases |
| Licensing | The exact custom license identified in section 3 for eligible first-party material; all third-party obligations preserved |

Use `G:\Program Files\Portable\Scripts\Numera-Core-Browser` as the default Windows project directory and `G:\Program Files\Numera Core Browser` as the default installed-program location. Support portable operation with explicit writable data/cache roots. Never assume a `G:` drive on GitHub runners or on other users' machines. Use PowerShell 7 for user-facing Windows orchestration; call native platform/upstream tools where required.

Do not ask the user to repeat these decisions. Do not silently reinterpret “owned core” as a launcher that downloads Camoufox, CloakBrowser, Patchright, or Playwright browsers and changes their product label.

## 2. Inspect before implementing; prove the build constraint first

Read the current Numera Browser repository, its selected runtime, open migration work, applicable rules, and existing tests. The reference examined for this handoff was branch `migration/native-runtime-checkpoint-20261012`, commit `d3b5127a8e467aa9a65a726f955ae51e7d4504d3`. Its README still describes Python as selected and Rust as a non-default candidate. Treat this as historical evidence to refresh, not permission to force that branch into production. [M1–M6]

### Mandatory feasibility gate

Before building a large feature framework, create and run a bounded source-build feasibility spike on allowed public GitHub-hosted runners. Record actual runner image, tools, free disk before/after safe cleanup, peak working set/disk, source acquisition size, compilation/linking time, artifact size, and cold-cache behavior for each proposed engine/platform combination.

GitHub currently documents `14 GB` SSD for standard public runners and a `6-hour` hosted job limit; Chromium's Windows build instructions require at least `100 GB` free disk. These facts make complete multi-platform engine builds a serious unresolved resource constraint, not an item to hide in a workflow file. Recheck official documentation and measure the actual selected runner. [S1–S3]

Investigate supported reduced-symbol builds, bounded parallelism, minimal required targets, safe cleanup, verified incremental artifacts, and DAG partitioning. Do not disable sandboxing or security features to make a build fit. Any split-build scheme must prove a cold bootstrap, correct dependency invalidation, provenance for every intermediate, and a final link/package step that fits a permitted job. A warm cache alone is not a reproducible build strategy. Keep source/build workspaces separate from the portable runtime directory; Chromium's source instructions impose path constraints, including avoiding spaces, which the orchestration must accommodate without moving the user's project silently. [S3]

Do not label an upstream binary download, an artifact build that reuses precompiled engine objects, or repackaging a CEF distribution as a full source build of Numera's patched renderer. Such assets may serve explicitly labeled development baselines; they do not satisfy owned-engine release acceptance. Do not assume resource limits can be bypassed by adding retries or unlimited cache storage.

If a target remains blocked, record **BLOCKED** with measured evidence and the exact failing stage. Continue independent Rust, contract, test, and integration work, but do not certify the full requested Core as ready, publish fake release badges, switch to paid infrastructure, or silently drop required engine/platform scope. A real infrastructure blocker must remain visible in the final implementation report. The specification does not guarantee that the complete requested source-build matrix will fit free runners.

## 3. License and source provenance

Use the license at the user's exact selected URL:

`https://github.com/KiaroSama/Numera-Browser/blob/migration/native-runtime-checkpoint-20261012/LICENSE`

The inspected immutable reference is [M2], Git blob `373e1b5b82fb98ca689731c206faa0661153941a`. Its title is **Noncommercial Network Share-Alike License — Version 1.0**, with identifier **`LicenseRef-Noncommercial-Network-ShareAlike-1.0`**. Preserve its text; do not substitute the older proprietary default-branch license, GPL, or a generic MIT template. Apply it only to material the relevant rights holders can license this way.

Maintain `LICENSE`, `THIRD_PARTY_NOTICES`, a machine-readable license inventory, source locations, modification notices, and build-matched complete-source releases. Identify this custom-licensed first-party project as source-available/noncommercial rather than claiming OSI-approved open-source status. Public repository visibility does not settle license compatibility.

Audit every copied patch, Rust dependency, native bridge, browser component, bundled data set, font, extension, and binary. Preserve upstream MPL file-level requirements and applicable WebKit LGPL/BSD obligations; do not place a conflicting noncommercial restriction on upstream-covered files. Copied or adapted source retains all applicable conditions. A source-code boundary or IPC boundary is not automatically a legal exemption. Do not incorporate a component when compatibility or redistribution permission is unresolved. [S8–S9]

CloakBrowser's wrapper and distributed browser binary have different rights; its private patches are not available merely because the wrapper is public. Study public interfaces and independently implement appropriate behavior without redistributing restricted binaries or fabricating unavailable patches. Clearcote also distinguishes its public source/build from newer private offerings; use only verifiably licensed public material. Keep reference-only entries distinct from adopted dependencies. [R4–R5]

Do not scrape commercial products to obtain private code, remove their license checks, or imply their endorsement. Build improvements from licensed implementations, standards, independent design, and reproducible tests. Preserve third-party trademarks only as truthful attribution.

## 4. Architecture and source organization

Use a Rust workspace with explicit responsibilities. The following crate names are proposed implementation boundaries, not pre-existing APIs:

| Area | Responsibility |
|---|---|
| `core-contracts` | Canonical schemas, Rust types, capability/version negotiation, golden fixtures |
| `core-runtime` | Launch planning, engine selection, lifecycle, cancellation, effective configuration |
| `core-process` | OS ownership, admission grants, supervision, crash recovery, safe detach |
| `core-engines` | Chromium/Gecko/WebKit adapters and native bridge interfaces |
| `core-identity` | Typed device modes, coherent identity policies, deterministic validation |
| `core-network` | Authorized proxy transport, relay where necessary, DNS/WebRTC policy diagnostics |
| `core-updater` | Trusted metadata, downloads, staging, activation, rollback, leases and GC |
| `core-automation` | Protocol gateways, endpoint authorization, protocol-specific capability mapping |
| `core-cli` | Human and structured CLI, headless/server operation, diagnostics |
| `core-gui` | Actual browser integration/chrome and shared theme support |

Keep upstream source checkouts or references, patch queues, build recipes, and component licenses explicitly separated from first-party application code. Pin upstream commits, toolchains, dependency locks, patch-series hashes, and build configuration. Give each patch a rationale, affected engine/version, regression test, and rebase/removal condition. Prefer small, reviewable engine changes over a permanently diverging monolithic fork.

Share behavior between CLI, GUI, and manager through the runtime, not copied handlers. Every nonvisual capability must have CLI/API parity. An optional graphical package must not cause a headless CLI installation to initialize a display server or download all engines. Standard Core use must not require user-installed Node.js or Python; they remain acceptable upstream build tools and external automation-client runtimes. Native C++/Objective-C/JavaScript glue is allowed where engines require it, but do not hide the real runtime in Python/TypeScript and call the Rust launcher the core.

Report language composition honestly: first-party authored source separately from generated, vendored, and upstream source. Do not pad Rust files or manipulate language statistics to disguise engine-language reality. No majority-Rust claim over all Chromium/Firefox/WebKit source is required or credible merely from the workspace choice.

## 5. Engine families and real browser functionality

Produce separately versioned `numera-chromium`, `numera-gecko`, and `numera-webkit` engine packages. Each package records its actual upstream revision, Numera patch revision, target triple, build mode, source archive, licenses, hashes, and supported features. Core version and upstream browser version must not be conflated.

The default Chromium experience must be a real browser window with tabs, navigation/address bar, back/forward/reload, downloads, permissions, certificate/security UI, bookmarks/history where supported, session restore, developer tools, and extension management appropriate to that engine. Keep equivalent ordinary-browsing behavior for the other families where implemented; expose honest differences. Retaining and modifying upstream browser chrome is acceptable. A Rust-rendered toolbar hosting the actual selected engine is also acceptable if it preserves isolation and capabilities.

Do not build a Tauri/WebView2/WKWebView shell and present its system webview as Numera Chromium, Gecko, or the portable WebKit fork. A settings window is not the requested browser GUI. CEF and `cef-rs` are optional architectural references, not a mandate: verify actual platform, embedding, extension, automation, and upstream-update limitations before adoption. CEF prebuilt packages do not magically include Numera-specific native patches. [R8]

Retain multi-process isolation, the browser sandbox, site isolation where provided, HTTPS verification, safe permission boundaries, and crash handling. No default `--no-sandbox`, certificate-error suppression, globally disabled web security, or publicly bound debugging ports. Audit upstream services individually: remove unwanted telemetry/accounts only where appropriate, while preserving or explicitly replacing security functionality. Do not strip protective features indiscriminately under the label “ungoogled.” [R9]

Extensions are engine-specific. Do not promise Chrome extensions on Gecko/WebKit. Implement permission-aware install/uninstall, isolation, update provenance, headless support tests, and unsupported-status reporting. Do not bundle proprietary DRM/CDM components, codecs, fonts, or commercial browser binaries without appropriate rights. Document media support by actual build, not a copied upstream marketing list.

## 6. Android and iPhone-like desktop emulation

AdsPower's documented Android/iOS profile settings are a product reference for the requested desktop experience, not evidence of its private implementation or proof that a JavaScript patch turns a desktop engine into a real phone. Build explicit **Android-emulated** and **iOS-emulated** modes. Native mobile apps, real-device farms, and cross-device syncing are not part of this initial scope. [S4]

Use a typed, versioned device catalog with provenance and internal consistency rules. Coordinate viewport/screen size, device scale factor, orientation, touch/pointer/hover behavior, language/timezone, media queries, relevant navigator values, browser family/version, network-facing metadata, and supported graphics/media capabilities. Seed profile identity deterministically and keep it stable across tabs, workers, restarts, and supported updates. Never randomize each observable independently on every page load.

Apply settings at the lowest appropriate supported layer: engine-native controls first, protocol controls where they are accurate, narrowly scoped JavaScript only when necessary and tested. Audit consistency across top-level pages, same/cross-origin frames, dedicated/shared/service workers, popups, and newly created targets. Unsupported hardware/OS APIs must be honestly absent or reported as limited, not replaced with implausible contradictory responses.

The mobile mode must affect actual web behavior where promised, not just the UA string or a phone-shaped window. Keep headers and page-visible values coherent, including request-triggered Client Hints behavior for supporting families. Record limitations in the effective configuration report. Desktop Chromium with an iPhone UA is not Safari; a WebKit build with mobile settings is still not Apple's complete Safari/iOS stack. Do not claim genuine iOS TLS, platform APIs, device attestation, media pipelines, or an undetectable identity from desktop emulation alone. [S5]

Offer mobile mode choices through GUI, CLI, and manager capability metadata. In strict mode reject impossible combinations; in permissive testing mode require explicit acceptance and retain warnings. Do not silently rewrite the profile's selected engine to make a combination appear supported.

Measure outcomes on controlled fixtures and authorized test sites. No “100% undetectable,” universal CAPTCHA success, or guaranteed account-safety claims. Any human-input simulation feature is an explicit test facility with deterministic replay; it must not silently perform account activity or become a release gate dependent on defeating third-party protections.

## 7. Automation compatibility without protocol confusion

Playwright is an automation framework, Patchright is an altered Playwright path, and Camoufox/CloakBrowser are browser-derived ecosystems. Extract their useful engineering ideas, not four inseparable runtime dependencies. Maintain an evidence-backed compatibility matrix for actual client versions, operating systems, engines, persistent profiles, extensions, and headed/headless modes. [R1–R4]

| Engine | Playwright | Puppeteer | Selenium |
|---|---|---|---|
| Chromium | Test `connectOverCDP` as a lower-fidelity path; implement/version-pair a proper Playwright-compatible transport for features not available through it | Real CDP attachment and navigation/network/target tests | Real compatible ChromeDriver/WebDriver path, including supported existing-session attachment behavior |
| Gecko | A genuine Playwright-compatible browser/transport path; generic BiDi alone is not Playwright protocol compatibility | Test supported WebDriver BiDi client functionality | GeckoDriver/Marionette and supported BiDi functionality |
| WebKit | A tested Playwright-compatible engine/transport path | Do not invent native Puppeteer WebKit support; expose as unsupported unless a separately implemented adapter is proven | Do not label a custom port SafariDriver; specify unsupported combinations or an explicitly tested custom WebDriver adapter |

Playwright documents CDP attachment as Chromium-only and lower fidelity, while its native connection protocol has version-pairing requirements. A listening WebSocket is not proof of Playwright compatibility. Current Puppeteer supports Firefox through BiDi; that does not make all Chromium features portable. Consult current upstream documentation rather than implementing against stale protocol assumptions. [S6–S7]

The requirement to support all three automation tools does not authorize advertising every tool/engine combination as supported when the tool itself lacks it. Make supported combinations genuinely work; show limitations in API/GUI/docs and prioritize the user's actual manager workflows. Do not silently run a different engine for unsupported combinations.

Test discovery/attach, persistent contexts, target creation, navigation, selectors, input, script execution, screenshots, downloads/uploads, dialogs, permissions, frames/workers, network interception, timeouts, disconnect/reconnect, and orderly stop. Advanced native Playwright features need native-protocol tests; passing a single CDP screenshot is insufficient. Keep client runtimes optional and outside the mandatory Core runtime. A temporary external compatibility bridge must be separately packaged and clearly labeled; it cannot become an undocumented dependency or prove completion of a required Rust-native path.

## Shared integration contract — NCB-1

**This section is intentionally identical in the Core build handoff and the Numera Browser integration handoff.** It is the agreed initial design, not a claim that these interfaces already exist. The Core repository must implement and publish the canonical schemas and conformance fixtures. The manager consumes a pinned release of those artifacts instead of independently recreating them. Changes require a versioned contract update, migration notes, and passing producer/consumer tests.

### Identity, versions, and capability negotiation

| Item | Required initial definition |
|---|---|
| Product | `Numera Core Browser` |
| Executables | `numera-core` and separately installable `numera-core-gui`; platform suffixes apply |
| Manager provider/backend | `numera-core` |
| Engine families | `chromium`, `gecko`, `webkit` |
| Engine package identifiers | `numera-chromium`, `numera-gecko`, `numera-webkit` |
| Device modes | `desktop`, `android-emulated`, `ios-emulated` |
| IPC namespace | `numera.core` |
| Initial wire major | `1`; explicitly distinct from any existing manager adapter protocol numbered `1` |
| Contract distribution | A versioned `numera-core-contracts-v1` release artifact with hashes, schemas, fixtures, and changelog |
| Version dimensions | Core SemVer, wire protocol, schema revision, engine upstream revision/build digest, profile-data format, and theme-token revision are separate |

Begin every connection with `hello` and `capabilities.get`. Negotiate overlapping protocol/schema ranges and required capabilities before any profile mutation or browser spawn. Capabilities are scoped to the exact engine build, host platform, device mode, and automation transport. Each feature reports `supported`, `partial`, or `unsupported`, with structured limitations and evidence references. A missing or unrecognized capability is not support.

### Local transport and request handling

The baseline managed transport is inherited stdio carrying bounded UTF-8 NDJSON. Initial maximum frame size is `1 MiB`; large diagnostics or artifacts use explicitly authorized streams/files rather than unbounded frames. Stdout contains protocol frames only. Logs go to stderr or configured log files. Optional local named-pipe/Unix-socket and authenticated loopback HTTP/WebSocket transports must expose the same semantics, not a second independently evolving API.

Every request includes namespace, protocol major, request ID, operation, a bounded relative deadline, and a typed payload. Responses correlate the request ID and include either a typed result or a stable error object. Reject duplicate JSON keys, invalid encodings, oversized frames, unknown required fields, invalid enum values, and namespace mismatches. Specify forward-compatible optional fields separately. Use monotonic clocks for deadline accounting, bounded queues, backpressure, cancellation, and deterministic behavior after EOF. Do not compare monotonic timestamps from different processes.

Authenticate non-inherited transports. Validate local peer identity where supported, restrict filesystem/socket permissions, protect HTTP/WebSocket endpoints against hostile origins and DNS rebinding, and never expose raw browser control publicly by default. An unpredictable port is not authentication. Do not place tokens or proxy credentials in URLs, command lines, diagnostic exports, or logs.

### Operations and canonical artifacts

Implement these operation groups with shared validation and stable errors:

- `hello`, `capabilities.get`, `engines.list`, `engines.install`, `engines.verify`, `engines.remove`.
- `updates.check`, `updates.stage`, `updates.activate`, `updates.rollback`.
- `launch.plan`, `session.launch`, `session.status`, `session.list`, `session.attach`, `session.stop`, `session.events`.
- `theme.set`, `diagnostics.collect`.

Separate global package administration from session-scoped authorization. A browser session credential must not authorize arbitrary engine installation or another profile's control. Event streams carry sequence numbers and a resumable cursor where retained; report retention gaps and require a state resynchronization instead of silently losing events.

Publish JSON Schemas, protocol documentation, positive/negative golden fixtures, capability manifests, theme tokens, engine compatibility manifests, and an executable producer/consumer conformance harness. Publish actual release provenance and smoke-test evidence alongside them. Schema syntax validation alone is not interoperability testing.

### Launch snapshot and configuration ownership

`launch.plan` receives a typed, immutable snapshot: manager-owned profile ID; a unique launch/session ID; idempotency key; selected engine family and build policy; device mode; required capabilities; fingerprint seed and requested identity; startup URLs; resource and native-profile-directory grants; extension references; session lifetime mode; proxy policy and separately delivered secret references; and browser-chrome theme tokens.

A plan is side-effect free: no browser launch, package update, destructive storage preparation, or external network probe. Any necessary probe is an explicit, separately authorized operation performed before final planning. Planning returns the exact selected build, normalized effective settings, rejected/unsupported settings, required migration/preparation steps, and policy warnings. Nothing silently switches engine families, replaces a required feature, or falls back to a direct network connection.

The manager owns profile business records, credentials at rest, folders/tags, scheduling, and user decisions. Core owns the application of engine-specific settings to the running browser and its session-scoped native resources. The manager must not additionally inject a competing fingerprint script, start a duplicate proxy relay, or apply a second set of Client Hints overrides for a Core-backed session.

### Admission, process ownership, and lifecycle

Acquire admission before spawn and establish durable ownership before returning a successful launch. Integrate the existing manager lease/claim mechanism through an explicit ownership grant; do not create two conflicting authorities or acquire the same non-reentrant lock twice. The implementation must document the single logical owner and the exact OS handle/lease holder in every state.

Managed mode preserves the manager's existing admission authority; Core enforces its granted session and controls the browser process tree inside that boundary. Standalone mode uses Core's own equivalent admission system. A detached session requires a surviving supervisor/lease holder before the original owner exits. Never release a profile merely because the launching client disappeared while its browser descendants remain alive.

A launch ID plus idempotency key cannot create two browsers when a response is lost. Lifecycle states include `planned`, `admitted`, `starting`, `running`, `stopping`, `stopped`, and explicit failure/recovery-required states. Cancellation during startup must clean up all owned descendants and native resources before releasing admission. Ambiguous/corrupted ownership records fail closed. Validate process identity using more than a reusable PID. Test Windows job objects and POSIX process ownership separately; do not assume a process group alone prevents every escaped descendant.

A running response returns a durable session handle, exact engine build/digest, the effective configuration report, and the genuinely available automation endpoints with their protocol/version and attachment limitations. Status after reconnect must reconcile with durable state and OS ownership, not only an in-memory map. Stopping one session must not stop unrelated profiles.

### Data, updates, and rollback

Treat Chromium, Gecko, and WebKit native profile directories as different storage formats. Do not rename or copy one into another and promise session compatibility. Migration is explicit, capability-dependent, copy-first, and non-destructive. Keep application records separate from version-sensitive browser databases. A browser binary rollback is allowed only with a compatible native profile or an appropriate pre-upgrade snapshot.

Core's updater is the single installer/activator for its engine packages. The manager requests actions and displays status; it does not maintain a competing updater. Signed compatible releases may be staged automatically. Do not replace in-use executables, close active sessions, reboot the operating system, or silently change a pinned profile's engine. Keep old packages while leased; garbage collection must honor active and recoverable sessions. Recovery from power loss must converge to a verified old or new package, never a partially installed active package.

Stable errors must distinguish at least: protocol mismatch, unsupported capability, incompatible engine/data format, missing package, ownership conflict, admission timeout, authentication failure, proxy failure, invalid configuration, update signature failure, insufficient resources, launch failure, cancellation, and recovery required. Display actionable errors without leaking secrets.

**Contract acceptance:** both repositories must pass the same fixtures, real process-lifecycle tests, version-skew tests, and a paired real-browser smoke suite. A mocked server or a matching JSON example is not sufficient.

## 8. Theme, interaction, and UI consistency

The inspected manager theme is centralized in `dashboard/src/theme.ts` and contains dark/light palettes, semantic ramps, and an ink-violet identity. Refresh that file and relevant branding at implementation time; do not invent a separate visual identity. Extract a small neutral, versioned design-token artifact shared through the contract rather than importing React internals into browser-native code. [M3]

Support dark, light, and system modes; profile color/avatar indicators where the manager supplies them; accessible keyboard focus; high-DPI layouts; localization; and correct RTL layout. Use Latin digits `0–9` and a suitable Latin numeric font with bidirectional isolation for IDs, paths, versions, and addresses. Do not mirror URLs or break mixed Persian/English labels. Browser-chrome theme and a web page's `prefers-color-scheme` are different settings: changing the manager theme must not accidentally mutate a profile's website-visible identity.

Browser UI must include exact build/family, emulation status and limitations, update state, and safe diagnostics. Use existing properly licensed brand assets only where authorized. Do not download arbitrary fonts at runtime or redistribute font files without verifying their licenses.

## 9. Network, privacy, storage, and security

Keep manager-owned proxy credentials encrypted at rest in the manager's vault. Core receives only what is needed for an authorized session through a protected channel. For standalone operation provide an explicit credential-store design; do not copy the manager's entire Vault implementation into a second synchronized database.

Implement and test HTTP/HTTPS/SOCKS proxy behavior, authentication, remote DNS policy, IPv4/IPv6, WebSocket traffic, bypass rules, connection failure, and cancellation. Use a session-owned loopback relay only where an engine needs it. If Core owns the relay, the manager must not start another. Never fail open to a direct connection when a required proxy fails. Keep origin TLS validation intact. Test WebRTC/DNS policies with controlled endpoints, not a browser-setting label alone.

Do not confuse network identity with JavaScript identity. TLS handshake behavior, HTTP protocol negotiation, browser features, and OS capabilities need their own evidence. Avoid broad TLS impersonation promises without actual engine-network implementation and testing. A proxy's success response is not proof that its exit IP changed.

Native profile data, downloaded assets, package staging, and crash dumps have explicit ownership and permissions. Defend path handling against traversal, symlinks/reparse points, archive extraction escapes, oversized/decompression-bomb assets, conflicting case normalization, and malicious manifest paths. Validate engine binaries and bridges before execution. Keep credentials, cookies, storage tokens, browsing content, and full sensitive URLs out of ordinary logs and CI artifacts.

Use threat modeling and tests for hostile web pages reaching local services, malicious extensions, compromised update metadata, untrusted PR builds, concurrency races, and stale process ownership. Fuzz parsers and IPC/state boundaries; justify and review every unsafe Rust block. Use licensed, current dependencies with vulnerability and license review.

## 10. GPU-first performance with evidence

Preserve upstream GPU acceleration for compositing, supported media decoding, WebGL and WebGPU where available. Prefer the actual host-supported accelerated path; do not globally disable the GPU to simplify automation or fingerprint control. Record actual acceleration backend, software fallback, and failure reason in diagnostics without falsely reporting a phone GPU as proof of physical hardware.

Measure startup latency, idle/active memory, CPU/GPU utilization where observable, input-to-render responsiveness, multiple-profile overhead, navigation throughput, and update size against equivalent unmodified engine baselines on the same machine/build mode. Set regression budgets after baselining, not invented universal speedup numbers. Public hosted runners may exercise software graphics paths; such results are not proof of hardware acceleration on every supported physical GPU. Mark unmeasured hardware claims explicitly and never commission paid GPU runners without authorization.

Use shared immutable engine packages across profiles, bounded concurrency, lazy optional services, and caches keyed by genuine compatibility. Do not share cookies, authentication state, writable native databases, or identity state merely to reduce memory. Avoid unsafe overcommit that destabilizes all sessions.

## 11. Packaging, portability, and installation

Publish separate CLI-only, GUI-plus-runtime, and optional per-engine packages. The CLI package must run diagnostics/package management without a display; a GUI launch requires an installed compatible engine. Provide a first-run component chooser and noninteractive equivalents, truthful download sizes, offline installation from verified packages, and idempotent repair/removal. Do not automatically fetch every engine when the user selected only one.

Portable mode keeps writable data, logs, temporary files, and caches under the explicitly selected portable root where feasible, with clear handling for OS-managed storage. Installed mode uses appropriate per-user data directories when the program directory is read-only. Test spaces, Unicode/Persian paths, long paths, relocation, missing drives, no-admin accounts, disk-full conditions, interrupted extraction, and upgrades preserving existing data. Never modify unrelated global PATH or PowerShell execution policy as a shortcut.

GitHub runners use their own workspace/scratch directories. Build in native platform jobs and verify each executable on its actual target platform; a cross-compiled artifact is not a native runtime test. Distinguish application update signatures, GitHub artifact attestations, Windows Authenticode, and Apple signing/notarization. Free CI does not automatically provide commercial signing credentials. Produce honest signed/unsigned status; do not claim notarization or spend money when credentials are unavailable.

## 12. Upstream tracking and safe automatic updates

Track official Chromium stable/security revisions, Firefox stable and any deliberately supported ESR track, WebKit upstream revisions, and relevant automation protocol clients separately. Do not define “latest” by the release date of a wrapper package. Use official version feeds and source metadata, record the resolved versions at each run, and never hard-code a guessed current version in a permanent workflow. WebKit does not have a universal standalone Safari-equivalent release number. [S10–S12]

Implement a proposed `6-hour` scheduled watcher, plus manual dispatch. Treat prompt discovery and a candidate update within `24 hours` when dependencies/build capacity permit as an engineering target, not a guaranteed SLA. Scheduled Actions can be delayed and builds can fail. Report upstream lag, patch conflicts, missing binaries, failed tests, and security urgency openly.

The watcher creates or updates one idempotent candidate PR per coherent update, rebases reviewed patch queues, regenerates compatibility metadata, runs source/real-browser tests, and produces evidence. Do not auto-merge or close PRs/issues. User-approved release promotion and installed-client automatic updates are distinct operations: automation may stage a candidate; it must not publish an unreviewed engine as trusted stable by bypassing the project's rules.

Use signed, expiring, rollback-resistant update metadata with a documented trust root and rotation/revocation process; evaluate a maintained TUF implementation such as `tough`. Use HTTPS, cryptographic artifact hashes, provenance, and least-privileged release signing. A hash delivered by the same compromised unauthenticated source is insufficient. Pin dependencies and GitHub Actions by reviewed immutable revisions. Do not expose signing keys to PR workflows. [R15; S13]

Install into versioned staging directories, verify before activation, use atomic pointer/manifest transitions with crash recovery, and keep active-session leases. Updates activate for eligible new sessions, never by killing current browsing. Preserve engine pins and schema/data compatibility; an application binary downgrade does not make already-upgraded browser databases safe. Implement a rollback test with real data snapshots. Offline/expired-metadata behavior must be explicit and secure, not an undocumented signature bypass.

## 13. Logging, CLI behavior, and diagnostics

Every implemented script and application component uses **INFO, WARNING, ERROR, DEBUG** primary levels. Every entry includes an unambiguous timestamp/timezone, severity, component, and meaningful message; add safe request/session correlation identifiers. Keep CLI `--json` and IPC stdout machine-readable. Send diagnostics to stderr/files. Redact secrets before all sinks, including dependency/engine stderr, and fuzz redaction boundaries with synthetic tokens.

Provide deterministic exit codes, help, JSON output, bounded waits, cancellation, and a `doctor`/diagnostic command. Export sanitized capability/build/ownership/update evidence with explicit opt-in for any potentially sensitive payload. Never print proxy passwords, API tokens, cookies, vault master passwords, or full credential-bearing command lines. Rotation/retention policy is explicit and configurable; do not erase user evidence unexpectedly.

## 14. CI and regression acceptance matrix

Run source/contract checks on PRs, targeted real-browser checks where changes demand them, and broader scheduled/release suites. All build/test jobs use the authorized free hosted infrastructure. Use concurrency groups, correct caching, path-aware matrices, bounded timeouts, and retry only for diagnosed transient failures. Never convert required tests to `continue-on-error`, skipped jobs, or mocked green checks to publish a release.

| ID | Required suite and acceptance evidence |
|---|---|
| C01 | Rust formatting, strict linting, unit tests, MSRV/current supported toolchain policy, dependency/license/security checks |
| C02 | Canonical contract schema, positive/negative fixtures, namespace/version skew, duplicate keys, frame limits, backpressure and EOF |
| C03 | Model/property tests for identity consistency, stable seed behavior, invalid device/engine combinations |
| C04 | Real CLI/GUI/runtime capability parity; no nonvisual GUI-only operation |
| C05 | Engine patch application/rebase checks and native regression tests against pinned sources |
| C06 | Cold and warm source builds, final link/package feasibility, reproducible provenance per required platform |
| C07 | Native package smoke tests on Windows, Linux, macOS; fresh machine without user Python/Node; optional-component combinations |
| C08 | Real browser navigation, JS/DOM/CSS behavior, frames/workers, permissions, storage, downloads, crash/session restore |
| C09 | Targeted WPT subsets mapped to modified behavior, baseline comparison and documented expected failures |
| C10 | Headed/headless comparisons, extensions where supported, actual browser GUI interaction and accessibility |
| C11 | Desktop/Android/iOS emulation fixtures checking page and request behavior, new targets and persistence |
| C12 | Real Playwright client/protocol cases; CDP-only and native-protocol evidence kept separate |
| C13 | Real Puppeteer CDP/BiDi supported combinations; unsupported combinations rejected truthfully |
| C14 | Real Selenium/driver compatibility, lifecycle, existing-session attachment where supported |
| C15 | Proxy auth/failure, DNS/WebRTC/IPv6 policy, TLS validation, no silent direct fallback, one relay per session |
| C16 | Concurrent same/different-profile launches, lost launch replies, duplicate retries, admission timeout, single-session stop |
| C17 | Startup cancellation, manager/Core/browser crash, descendant cleanup, detach, PID reuse, corrupted claims, restart recovery |
| C18 | Signed updater failures, expired/revoked metadata, tampered archives, disk-full/power-loss, active-session leases and rollback |
| C19 | Cross-engine migration rejection, same-family data-format checks, copy-first migration, snapshot recovery without source corruption |
| C20 | Path/Unicode/RTL/Latin numerals, theme compatibility, read-only installations, portable relocation and no-admin use |
| C21 | IPC/parser fuzzing, sanitizer/native tests where feasible, unsafe review, malicious web-origin local API tests, secret-redaction tests |
| C22 | Paired integration with real Numera Browser, version skew, real engine artifacts, update staging and preserved sessions |

Do not treat public anti-bot challenge pages as deterministic mandatory tests or claim all websites were tested. Use controlled local servers and consented fixtures. Browser source compilation, physical-GPU testing, notarization, and real-engine integration must each retain their own PASS/FAIL/BLOCKED/NOT_RUN status. A scaffold or mocked adapter cannot stand in for a missing engine.

Publish a machine-readable coverage/evidence matrix mapping requirement IDs to test names, runner/OS, exact commit/build, result, and artifact link. Include duration and diagnostic context for failures without secret-bearing data. Preserve meaningful known limitations and security regressions as release blockers.

## 15. Implementation sequence and completion rules

**Gate A — repository, license, baseline, feasibility.** Inspect rules/current manager; create or continue the exact public repo; establish the license inventory; run the bounded real source-build spike; publish measured infrastructure status. Do not spend the session only writing architecture documents.

**Gate B — working vertical slice.** Implement canonical contracts, actual Rust CLI/runtime, one real owned Chromium browser launch, GUI interaction, ownership cleanup, structured diagnostics, and real Playwright/Puppeteer/Selenium tests on supported Chromium paths. A downloaded stock binary may test a development baseline but cannot pass the owned-engine gate.

**Gate C — breadth without false equivalence.** Implement the Gecko and WebKit engine paths, mobile modes, modular packaging, theme tokens, and accurate per-tool/platform capability mapping. Keep unsupported combinations explicit rather than replacing engines silently. Required engine/platform blockers remain unresolved requirements, not deleted scope.

**Gate D — updates and integration candidate.** Complete signed update staging/activation/rollback, real data compatibility tests, canonical contract artifacts, and a Numera Browser paired smoke fixture. Publish an integration-ready prerelease only when its declared support has passed real acceptance.

**Gate E — release acceptance.** Run the required platform/engine/test matrix, audit source/notice delivery, compare performance baselines, verify portable installation, and produce a requirement-by-requirement final report. Fix newly discovered bugs and add regression tests in the same implementation cycle. Re-run affected tests plus a final integrated suite; do not get stuck repeatedly rewriting the plan or marking the same unfixed defect resolved.

Use the user's configured author identity; where the environment calls for it, the requested email is `Kiaro.Sama.Dev@gmail.com`. Preserve existing work, do not force-push unrelated history, do not create unnecessary parallel branches, and do not merge or close PRs/issues without user authorization. Create/update appropriate implementation PRs according to the user's actual Rules. Report exact commits, PRs, successful workflow runs, deliverables, blocked targets, and remaining limitations.

The final repository must contain working source, upstream/patch locks, build scripts, tests, canonical contracts, source and binary release manifests, license inventory, CLI/API/user documentation, architecture/security/update ADRs, the feasibility report, compatibility/evidence matrices, and a short integration checklist for Numera Browser. It must not claim “better than every reference” without comparative evidence.

## 16. Reference repositories: evaluate, combine selectively, and document decisions

These are researched candidate references, **not an instruction to import every dependency or a benchmark ranking**. Refresh the current source, maintenance status, licenses, supported versions, and security posture before adoption. For each, record `adopt`, `adapt`, `reference-only`, or `reject`, the exact revision, rationale, rights, and the tests proving the chosen capability. Search for additional maintained alternatives during implementation, but prevent unbounded research loops by using explicit acceptance criteria.

| Ref | Repository | Appropriate use and boundary |
|---|---|---|
| R1 | https://github.com/microsoft/playwright | Multi-engine automation, protocol design, fixtures and device descriptors; not itself a rendering engine |
| R2 | https://github.com/Kaliiiiiiiiii-Vinyzu/patchright | Chromium automation/compatibility patching patterns; not Firefox/WebKit support by association |
| R3 | https://github.com/daijro/camoufox | Public Firefox-native customization, coherent identity and packaging ideas; audit every copied patch's rights |
| R4 | https://github.com/CloakHQ/cloakbrowser | Public API/product behavior reference; separately inspect https://github.com/CloakHQ/cloakbrowser/blob/main/BINARY-LICENSE.md and https://cloakbrowser.dev/; private browser patches/binary redistribution are not granted by a wrapper license |
| R5 | https://github.com/clearcotelabs/clearcote-browser | Inspect public Chromium patch architecture and reproducible public build only; https://www.clearcotelabs.com/ distinguishes public and private offerings |
| R6 | https://github.com/mattsse/chromiumoxide | Rust CDP integration and typed browser-control patterns; not a complete multi-engine browser |
| R7 | https://github.com/vercel-labs/agent-browser | Agent-friendly CLI semantics, structured output, sessions, browser automation architecture; verify current Rust/runtime boundaries |
| R8 | https://github.com/chromiumembedded/cef and https://github.com/tauri-apps/cef-rs | Native Chromium embedding/Rust bindings; evaluate update lag, extension/automation parity, native build requirements and licenses |
| R9 | https://github.com/ungoogled-software/ungoogled-chromium | Maintainable Chromium patch queues and service decoupling; do not blindly remove security services |
| R10 | https://github.com/apify/fingerprint-suite | Coherent profile data/validation ideas; no promise that JS injection produces native-device equivalence |
| R11 | https://github.com/daijro/browserforge | Correlated browser/device generation and data modeling; inspect data provenance and runtime independence |
| R12 | https://github.com/brave/adblock-rust | Optional Rust content filtering; audit code and filter-list licenses separately |
| R13 | https://github.com/rebrowser/rebrowser-patches | Public automation-context patch patterns and regression references; verify maintenance and licensing |
| R14 | https://github.com/web-platform-tests/wpt | Standards-regression fixtures and baseline comparisons, especially for modified engine surfaces |
| R15 | https://github.com/awslabs/tough | Rust TUF client/reference for trustworthy updates; verify current advisory fixes and supported metadata behavior |
| R16 | https://github.com/servo/servo | Rust/native browser architecture research only; do not pivot this agreed upstream-derived project into a new renderer |

### Official documentation and product references

- **S1:** https://docs.github.com/en/actions/reference/runners/github-hosted-runners — allowed runner specifications; verify at implementation time.
- **S2:** https://docs.github.com/en/actions/reference/limits — execution/storage/concurrency limits.
- **S3:** https://chromium.googlesource.com/chromium/src/+/main/docs/windows_build_instructions.md — real engine build prerequisites and source-path constraints.
- **S4:** https://help.adspower.com/docs/creating_browser_profiles and https://www.adspower.com/glossary/docs/device-emulation — product-level mobile profile/emulation behavior, not private internals.
- **S5:** https://playwright.dev/docs/browsers and https://playwright.dev/docs/emulation — engine and emulation distinctions.
- **S6:** https://playwright.dev/docs/api/class-browsertype — actual connection protocol and CDP limitations.
- **S7:** https://pptr.dev/webdriver-bidi and https://firefox-source-docs.mozilla.org/remote/ — Puppeteer/Firefox protocol support and current Firefox remote-control architecture.
- **S8:** https://www.mozilla.org/en-US/MPL/2.0/FAQ/ — MPL boundaries; not a substitute for component-specific review.
- **S9:** https://webkit.org/licensing-webkit/ — WebKit component licensing.
- **S10:** https://versionhistory.googleapis.com/v1/chrome/platforms — Chrome VersionHistory discovery entry; resolve the appropriate version endpoints.
- **S11:** https://product-details.mozilla.org/1.0/firefox_versions.json — Firefox release metadata.
- **S12:** https://github.com/WebKit/WebKit — WebKit upstream source/revisions.
- **S13:** https://theupdateframework.io/ — update trust and metadata design.
- **S14:** https://gologin.com/blog/meet-the-new-antidetect-browser-orbita/ — manager/browser product-boundary reference, not a source-code license.

### Inspected manager reference points

The following immutable URLs document the planning baseline, not an instruction to overwrite newer manager work:

- **M1:** https://github.com/KiaroSama/Numera-Browser/blob/d3b5127a8e467aa9a65a726f955ae51e7d4504d3/README.md — runtime/migration status and product boundary.
- **M2:** https://github.com/KiaroSama/Numera-Browser/blob/d3b5127a8e467aa9a65a726f955ae51e7d4504d3/LICENSE — the selected custom license.
- **M3:** https://github.com/KiaroSama/Numera-Browser/blob/d3b5127a8e467aa9a65a726f955ae51e7d4504d3/dashboard/src/theme.ts — existing design tokens and appearance.
- **M4:** https://github.com/KiaroSama/Numera-Browser/blob/d3b5127a8e467aa9a65a726f955ae51e7d4504d3/engine-rust/src/browser/adapter.rs — current native adapter framing, process admission and I/O.
- **M5:** https://github.com/KiaroSama/Numera-Browser/blob/d3b5127a8e467aa9a65a726f955ae51e7d4504d3/engine-rust/src/browser/launch.rs — existing backend dispatch and preparation responsibilities.
- **M6:** https://github.com/KiaroSama/Numera-Browser/blob/d3b5127a8e467aa9a65a726f955ae51e7d4504d3/engine-rust/src/browser/mod.rs — current native browser module boundaries.
