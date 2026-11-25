# Native browser SDK fixture

This fixture exercises the pinned Selenium WebDriver, Puppeteer, and Playwright
packages through the Node API from Rust. The adapter runs only the local browser
and local document supplied by Numera's native automation harness. Reports record
client compatibility after all three SDK scenarios and both viewport layouts pass.

Build the two targets with the pinned Rust toolchain.

```sh
cargo build --locked -p core-sdk-fixture
```

Copy the built `libcore_sdk_fixture.so` on Linux or `core_sdk_fixture.dll` on
Windows byte for byte to a path ending in `.node`. Keep the pinned Node runtime
and the SDK dependencies from `tests/native-automation/package-lock.json`
available to the runner. The adapter's pinned argument list is the absolute Node
path, the absolute `.node` path, and the absolute SDK `package.json` path.
The harness appends the driver address, executable, profile path, local fixture
URL, and report path. Pass the adapter specification through
`NUMERA_SDK_ADAPTER`.

Node loads the native library before evaluating an empty bootstrap. The Rust
library dispatches SDK calls and awaits their promises. The function bodies sent
to the browser cover only the controlled DOM assertions and viewport readings.
The reported headless coverage is independent of Numera's production browser
launch and automation capabilities.
