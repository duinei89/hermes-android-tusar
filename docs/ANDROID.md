# Android integration guide

This guide shows how to use `libtusar.so` from a Kotlin or Java Android app.
The library is a JNI wrapper around the Rust `hbc-decomp` engine. It reads a
Hermes bytecode file (`.hbc`), runs analysis, and returns results through a JSON
command protocol.

## Before you start

You need:

- An Android app using Kotlin or Java.
- A supported ABI artifact from a `hermes-android-tusar` release.
- A Hermes bytecode file that you are authorized to inspect.
- A writable app-private directory for the input HBC and optional cache/output
  files.

The upstream engine provides opcode tables for HBC versions 40 through 99.
When an exact table is unavailable, the parser may fall back to the nearest
older supported table, so always inspect the version returned by `info` and
record the release manifest's upstream commit when reproducibility matters.

## Download a release

Releases are published at:

<https://github.com/duinei89/hermes-android-tusar/releases>

A release is built from one exact `hbc-decomp` commit. Download:

- `libtusar-arm64-v8a.so` for 64-bit ARM devices.
- `libtusar-armeabi-v7a.so` for 32-bit ARM devices.
- `libtusar-x86_64.so` for 64-bit x86 emulators.
- `libtusar-x86.so` for 32-bit x86 emulators.
- `libtusar-android-<commit>.zip` for the complete packaged set.
- `manifest.json` for provenance and build metadata.
- `SHA256SUMS` for integrity verification.

Do not rename the libraries. Android expects the filename `libtusar.so` inside
each ABI directory. The release asset names identify the ABI; the library's
internal filename remains `libtusar.so`.

## Verify the release

On a Unix-like shell:

```bash
sha256sum -c SHA256SUMS
```

On macOS, use:

```bash
shasum -a 256 -c SHA256SUMS
```

Inspect `manifest.json` before shipping:

```json
{
  "project": "hermes-android-tusar",
  "library": "libtusar.so",
  "upstream_repository": "https://github.com/SymbioticSec/hermes-decomp",
  "upstream_commit": "...",
  "android_ndk": "26.3.11579264",
  "architectures": ["arm64-v8a", "armeabi-v7a", "x86_64", "x86"]
}
```

## Package the `.so` files

Copy the files into the application module exactly like this:

```text
app/
└── src/
    └── main/
        └── jniLibs/
            ├── arm64-v8a/
            │   └── libtusar.so
            ├── armeabi-v7a/
            │   └── libtusar.so
            ├── x86_64/
            │   └── libtusar.so
            └── x86/
                └── libtusar.so
```

If the app only targets modern physical devices, `arm64-v8a` may be enough. Keep
all four when you need broad emulator and legacy-device support.

Android Gradle Plugin normally packages `src/main/jniLibs` automatically. No
CMake or `externalNativeBuild` block is needed for this prebuilt library.

## Kotlin wrapper

Create a small wrapper so the rest of the app does not manually construct JSON:

```kotlin
package com.tusar.hermes

import org.json.JSONObject

object HermesNative {
    init {
        System.loadLibrary("tusar")
    }

    external fun nativeVersion(): String
    external fun nativeOpen(path: String): Long
    external fun nativeClose(handle: Long)
    external fun nativeCall(
        handle: Long,
        operation: String,
        argumentsJson: String,
    ): String

    fun call(handle: Long, operation: String, arguments: JSONObject = JSONObject()): JSONObject {
        return JSONObject(nativeCall(handle, operation, arguments.toString()))
    }
}

class HermesSession(private val hbcPath: String) : AutoCloseable {
    private var handle: Long = 0

    fun open() {
        check(handle == 0L) { "Session is already open" }
        handle = HermesNative.nativeOpen(hbcPath)
        check(handle > 0) { "Unable to read or parse Hermes bytecode: $hbcPath" }
    }

    fun call(operation: String, arguments: JSONObject = JSONObject()): JSONObject {
        check(handle > 0) { "Session is not open" }
        return HermesNative.call(handle, operation, arguments)
    }

    override fun close() {
        if (handle > 0) {
            HermesNative.nativeClose(handle)
            handle = 0
        }
    }
}
```

Use it off the main thread:

```kotlin
lifecycleScope.launch(Dispatchers.Default) {
    HermesSession(hbcFile.absolutePath).use { session ->
        session.open()
        val info = session.call("info")
        Log.i("Hermes", info.toString(2))

        val function = JSONObject()
            .put("function_id", 42)
            .put("show_offsets", false)
        val source = session.call("decompile", function)
        saveTextToFile("function-42.js", source.getString("result"))
    }
}
```

## Java declaration

Java callers can use the same symbols:

```java
package com.tusar.hermes;

public final class HermesNative {
    static {
        System.loadLibrary("tusar");
    }

    public static native String nativeVersion();
    public static native long nativeOpen(String path);
    public static native void nativeClose(long handle);
    public static native String nativeCall(long handle, String operation, String argumentsJson);

    private HermesNative() {}
}
```

## Getting an HBC path

`nativeOpen` accepts a filesystem path, not an Android resource name and not an
`AssetFileDescriptor`. Copy an APK asset into an app-private file first:

```kotlin
fun copyAssetToCache(context: Context, assetName: String): File {
    val output = File(context.cacheDir, assetName.substringAfterLast('/'))
    context.assets.open(assetName).use { input ->
        output.outputStream().use { outputStream -> input.copyTo(outputStream) }
    }
    return output
}
```

For an externally supplied file, prefer a `ContentResolver` stream and copy it
to `filesDir` or `cacheDir`. Use canonical paths and avoid passing untrusted
path strings directly to write operations.

## Command protocol

Every command is a string plus a JSON object. A successful response has:

```json
{"ok":true,"result":...}
```

An unsuccessful response has:

```json
{"ok":false,"error":{"code":"...","message":"..."}}
```

Always check `ok` before reading `result`. Do not treat a JSON error as a
successful empty result.

### File and function inspection

```kotlin
session.call("info")
session.call("list-functions", JSONObject().put("limit", 100))
session.call("list-versions")
```

`info` returns the input path, HBC version, function count, string count, and
global function index. Function IDs are zero-based.

### Disassembly

```kotlin
session.call(
    "disasm",
    JSONObject()
        .put("function_id", 42)
        .put("show_offsets", true),
)
```

The result is a text disassembly with Hermes instruction names, operands, string
literals, offsets, and labels.

### Lightweight decompilation

```kotlin
session.call(
    "decompile",
    JSONObject()
        .put("function_id", 42)
        .put("show_offsets", false)
        .put("propagate", true)
        .put("simplify", true)
        .put("recover_structures", true)
        .put("resolve_closures", false),
)
```

Use this for quick single-function output. `resolve_closures` adds closure
context work and may be slower.

### Full-pipeline decompilation

```kotlin
session.call(
    "decompile-full",
    JSONObject().put("function_id", 42),
)
session.call("decompile-all")
```

The first full-pipeline call can be expensive because it analyzes the entire
bundle. It is cached for later calls in the same native session and can reuse an
`<input>.hdcache` file.

### Metro modules and dependencies

```kotlin
session.call("modules", JSONObject().put("limit", 200))
session.call(
    "deps",
    JSONObject().put("module_id", 12).put("depth", 5),
)
```

Module discovery uses the full pipeline. Module IDs and function IDs are
separate values; do not substitute one for the other.

### Cross-references

Search strings:

```kotlin
session.call(
    "xref",
    JSONObject().put("query", "login").put("kind", "string"),
)
```

Search references to a function ID:

```kotlin
session.call(
    "search-functions",
    JSONObject().put("query", "42").put("kind", "function"),
)
```

### Graphs

Control-flow graph as Graphviz DOT:

```kotlin
session.call("graphviz", JSONObject().put("function_id", 42))
```

Call graph as text or DOT:

```kotlin
session.call(
    "callgraph",
    JSONObject()
        .put("function_id", 42)
        .put("depth", 5)
        .put("dot", true),
)
```

### Dumps and closures

```kotlin
session.call("dump", JSONObject().put("kind", "functions"))
session.call("dump", JSONObject().put("kind", "strings"))
session.call(
    "dump-table",
    JSONObject().put("kind", "sections").put("json", true),
)
session.call("closures", JSONObject().put("function_id", 42))
```

Structural table kinds include `cjs-modules`, `regexp`, `obj-shapes`,
`function-sources`, `string-kinds`, `sections`, `big-int`, and `array-buffer`.

### Security and runtime helpers

```kotlin
session.call("secrets", JSONObject().put("show_full", false))
session.call(
    "frida-hooks",
    JSONObject()
        .put("module_id", 12)
        .put("output_dir", File(context.filesDir, "hooks").absolutePath)
        .put("exports", "login,default"),
)
```

Secret scan output is redacted by default. Keep `show_full=false` unless the
caller is authorized and the result is handled securely.

### Bytecode write operations

Emit HASM:

```kotlin
session.call("emit-hasm", JSONObject().put("function_id", 42))
```

Patch a string by ID:

```kotlin
session.call(
    "patch-string",
    JSONObject()
        .put("id", 17)
        .put("new_value", "replacement")
        .put("output_path", outputFile.absolutePath),
)
```

Patch by old value instead:

```kotlin
session.call(
    "patch-string",
    JSONObject()
        .put("old_value", "https://old.example")
        .put("new_value", "https://new.example")
        .put("output_path", outputFile.absolutePath),
)
```

Patch a function from HASM:

```kotlin
session.call(
    "patch-function",
    JSONObject()
        .put("function_id", 42)
        .put("hasm", hasmText)
        .put("output_path", outputFile.absolutePath),
)
```

Inject a no-op or log stub:

```kotlin
session.call(
    "inject-stub",
    JSONObject()
        .put("function_id", 42)
        .put("kind", "nop") // or "log"
        .put("output_path", outputFile.absolutePath),
)
```

Create a minimal HBC:

```kotlin
session.call(
    "create",
    JSONObject()
        .put("version", 98)
        .put("strings", JSONArray().put("global").put("hello"))
        .put("output_path", outputFile.absolutePath),
)
```

Write operations create a new output file. Keep the input immutable, validate
the generated file, and reopen it before querying the modified contents.

## Lifecycle and threading

- Call `nativeOpen` once per input session.
- Always call `nativeClose`, preferably with Kotlin `use`/`AutoCloseable`.
- Never use a handle after closing it.
- A native session is protected by a lock. Calls on the same handle are
  serialized, especially while building the full pipeline.
- Separate handles can be used for separate files.
- Do not perform full-bundle analysis on the Android main thread.
- Do not assume returned text is small enough for a UI `TextView` or logcat.

## Troubleshooting

### `nativeOpen` returns `0`

The current JNI ABI uses `0` for all open failures, including an unreadable
path, malformed HBC, and unsupported format. The native API does not currently
expose a detailed open-error string, so inspect the path and file with the
standalone upstream CLI when you need a diagnostic.

Check that:

1. The path exists and is readable by the app process.
2. The file is actually Hermes bytecode, not a plain JavaScript bundle.
3. The HBC version is supported by the release.
4. The matching ABI library is packaged in the APK.
5. The library is loaded with `System.loadLibrary("tusar")`.

### `UnsatisfiedLinkError`

Check the ABI directory and filename:

```text
src/main/jniLibs/<abi>/libtusar.so
```

Also check that the device ABI is included in the release and that no stale
library with the same name is being loaded from another module.

### Slow first full decompile

This is expected. The full pipeline processes all functions and may build a
cache. Use `decompile` for a quick function-level result, and run full analysis
on a worker thread.

### Output is truncated or causes memory pressure

Request one function or module at a time. Save results directly to a file and
avoid logging the complete output. A future streaming API can be added without
changing the existing command semantics.

## Security and authorization

The library is an analysis and bytecode-research tool. Only use it with apps,
bundles, credentials, and code you own or are authorized to test. Treat HBC
files and decompiled output as sensitive. Secret scanning can reveal tokens and
private endpoints; redact output and avoid sending it to untrusted telemetry.
