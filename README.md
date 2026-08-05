# hermes-android-tusar

> **Use the Hermes bytecode decompiler from an Android app through one small JNI library.**

`hermes-android-tusar` packages the Rust [`hbc-decomp`](https://github.com/SymbioticSec/hermes-decomp)
engine as an Android-compatible `libtusar.so`. It exposes parsing, disassembly,
decompilation, Metro analysis, cross-references, control-flow graphs, secret
scanning, HASM/bytecode editing, and Frida hook generation through a stable,
JSON-based JNI protocol.

[![Build](https://github.com/duinei89/hermes-android-tusar/actions/workflows/build-release.yml/badge.svg)](https://github.com/duinei89/hermes-android-tusar/actions/workflows/build-release.yml)

## What this is

This repository is the Android integration layer. The decompiler engine lives
in the upstream `hbc-decomp` crate; this project adds:

- Android JNI exports for Kotlin and Java.
- Safe opaque handles for multiple open HBC sessions.
- Lazy full-program analysis and on-disk analysis-cache reuse.
- A single JSON command entry point so an app or an AI agent can call many
  decompiler capabilities without adding a new JNI method for every feature.
- GitHub Actions builds for four Android ABIs.
- Reproducible GitHub Releases pinned to an exact upstream commit.

It is **not** a JavaScript compiler. The write operations edit Hermes bytecode
or HASM; they do not turn decompiled JavaScript back into a bundle.

## Supported capabilities

| Area | Operations |
|---|---|
| File and functions | `info`, `list-functions`, `list-versions` |
| Read | `info`, `list-functions`, `disasm`, `decompile`, `decompile-full`, `decompile-all`, `extract`, `modules`, `deps` |
| Analyze | `xref`, `search-strings`, `search-functions`, `graphviz`, `callgraph`, `closures`, `debug`, `dump`, `dump-table`, `bin-diff` |
| RE helpers | `secrets`, `frida-hooks` |
| Bytecode writing | `emit-hasm`, `asm`, `asm-check`, `patch-string`, `patch-function`, `inject-stub`, `create` |
| CLI-only | `tui` returns a structured `CLI_ONLY` error; use the equivalent JSON operations in Android |

The complete protocol, argument schema, examples, and agent guidance are in:

- **[Android integration guide](docs/ANDROID.md)** — install and call the library from Kotlin/Java.
- **[AI-agent integration guide](docs/AI-AGENTS.md)** — tool schemas, safe workflows, and agent instructions.

## Quick start for Android

### 1. Download a release

Open the [GitHub Releases](https://github.com/duinei89/hermes-android-tusar/releases)
page and download either the ABI-specific libraries or the Android ZIP archive.
Every release includes:

```text
libtusar-arm64-v8a.so
libtusar-armeabi-v7a.so
libtusar-x86_64.so
libtusar-x86.so
libtusar-android-<upstream-sha>.zip
manifest.json
SHA256SUMS
```

Use `manifest.json` to identify the exact `hbc-decomp` commit and NDK version
used to create the artifacts. Verify downloads with `SHA256SUMS` before shipping
them.

### 2. Put libraries in `jniLibs`

For an Android application, place each ABI library at the matching path:

```text
app/src/main/jniLibs/
├── arm64-v8a/libtusar.so
├── armeabi-v7a/libtusar.so
├── x86_64/libtusar.so
└── x86/libtusar.so
```

Then load it with:

```kotlin
System.loadLibrary("tusar")
```

### 3. Add the direct Kotlin facade

The repository includes a drop-in wrapper at
[`android/src/main/java/com/tusar/hermes/HermesNative.kt`](android/src/main/java/com/tusar/hermes/HermesNative.kt).
Copy it into your app (or use it as the source for your own package). It exposes
these direct methods:

```kotlin
val handle = HermesNative.openHermesFile(path)
val metadata = JSONObject(HermesNative.getMetadata(handle))
val functions = JSONObject(HermesNative.listFunctions(handle))
val disassembly = JSONObject(HermesNative.disassembleFunction(handle, 42))
val source = JSONObject(HermesNative.decompileFunction(handle, 42))
val strings = JSONObject(HermesNative.searchStrings(handle, "login"))
val references = JSONObject(HermesNative.searchFunctions(handle, "42"))
val cfgDot = JSONObject(HermesNative.getControlFlowGraph(handle, 42))
val secrets = JSONObject(HermesNative.scanSecrets(handle))
HermesNative.closeHermesFile(handle)
```

`generateFridaHooks` and `patchFunction` are direct methods too. Every direct
analysis/write method returns the same JSON envelope as `nativeCall`, so callers
can use one parser and one error policy. The facade also provides typed-looking
Kotlin helpers for every remaining command (`modules`, `extractModules`,
`binaryDiff`, `emitHasm`, `assembleFunction`, `checkAssembly`, `createFile`,
and more). Keep the generic `nativeCall` escape hatch for new upstream commands.

The exported class name is `com.tusar.hermes.HermesNative`; if you move the
Kotlin class to another package, the Rust JNI symbol names must be regenerated.

The `nativeVersion()` response is currently:

```text
libtusar.so loaded successfully
```

### 4. Open a bundle and call a command

```kotlin
val handle = HermesNative.nativeOpen(hbcFile.absolutePath)
check(handle > 0) { "Could not parse Hermes bytecode" }

try {
    val response = HermesNative.nativeCall(
        handle,
        "info",
        "{}",
    )
    println(response)
} finally {
    HermesNative.nativeClose(handle)
}
```

`nativeOpen` expects a real readable filesystem path. If the HBC is packaged as
an APK asset, first copy it to `filesDir` or `cacheDir`; native Rust code cannot
open an `AssetManager` path directly.

## Response contract

Successful calls return:

```json
{
  "ok": true,
  "result": {}
}
```

Failed calls return:

```json
{
  "ok": false,
  "error": {
    "code": "INVALID_ARGUMENT",
    "message": "Missing integer argument: function_id"
  }
}
```

The main error codes are `INVALID_HANDLE`, `INVALID_ARGUMENT`, `INVALID_JSON`,
`DECOMPILER_ERROR`, `WRITE_ERROR`, and `IO_ERROR`.

## Full-pipeline behavior

The fast `decompile` operation analyzes one function. The full operations build
an expensive `PipelineContext` lazily on first use. The native session stores
that context and reuses it for later calls. When possible, the upstream cache is
stored next to the input as:

```text
<bundle-name>.hdcache
```

For large bundles, run expensive calls off the Android main thread. Full-bundle
results can be very large; prefer function- or module-level calls and write
large returned strings to a file from Kotlin instead of displaying them all at
once.

## Important write-path behavior

Write operations always require an explicit output path. They produce a new HBC
file and do not silently overwrite the input. The current bridge clones the
session's parsed file for the write, so the open session continues to represent
the original input. Close and reopen the output file before analyzing a patched
image.

Use write operations only on bytecode you are authorized to inspect or modify.
Keep original files and verify every generated HBC before deploying it.

## Automated releases

The workflow in `.github/workflows/build-release.yml` runs:

- manually through `workflow_dispatch`; and
- **once daily at 03:17 UTC** through GitHub Actions schedule polling.

It checks `SymbioticSec/hermes-decomp` `main`, detects its full commit SHA, pins
the Cargo dependency to that exact SHA, runs workspace tests, builds all four
Android ABIs, creates checksums and a manifest, and publishes a release tagged:

```text
upstream-<full-commit-sha>
```

The tag makes the workflow idempotent: rerunning the daily poll does not create
a duplicate release for the same upstream commit. The ZIP is ready to extract
into an Android module's `src/main/jniLibs` directory; `SHA256SUMS` covers the
individual ABI libraries and metadata files, not the ZIP container itself. A
manual dispatch is useful
when you need to rebuild or check immediately instead of waiting for the next
poll.

## Build locally or in CI

A Rust toolchain, Android SDK/NDK, and `cargo-ndk` are required:

```bash
cargo install cargo-ndk --locked
cargo ndk \
  -t arm64-v8a \
  -t armeabi-v7a \
  -t x86_64 \
  -t x86 \
  -o ./jniLibs \
  build --release -p tusar
```

The GitHub Action is the canonical reproducible build because it installs the
pinned NDK (`26.3.11579264`), resolves the upstream commit, runs tests, and
packages release assets.

## Repository layout

```text
hermes-android-tusar/
├── .github/workflows/build-release.yml  # Daily upstream poll and release
├── tusar-ffi/Cargo.toml                 # cdylib crate definition
├── tusar-ffi/src/lib.rs                 # JNI exports and handle registry
├── tusar-ffi/src/session.rs             # Parsed HBC session state
├── tusar-ffi/src/commands.rs            # JSON command protocol
└── docs/
    ├── ANDROID.md                       # Public Android integration guide
    └── AI-AGENTS.md                     # AI-agent/tool integration guide
```

## Upstream project and license

- Engine: [`SymbioticSec/hermes-decomp`](https://github.com/SymbioticSec/hermes-decomp)
- Android bridge: [`duinei89/hermes-android-tusar`](https://github.com/duinei89/hermes-android-tusar)
- License: MIT; see [LICENSE](LICENSE).

Only analyze or modify applications and bytecode for which you have permission.
