# AI-agent integration guide

This document is for an AI agent, automation service, desktop companion, or
Android-hosted assistant that needs to drive Hermes bytecode analysis through
`libtusar.so`.

The library is intentionally exposed in two equivalent layers:

```text
Direct Kotlin JNI methods:
openHermesFile(path) -> handle
getMetadata(handle) -> responseJson
listFunctions(handle) -> responseJson
...
closeHermesFile(handle)

Generic escape hatch:
nativeOpen(path) -> handle
nativeCall(handle, operation, argumentsJson) -> responseJson
nativeClose(handle)
```

The direct methods are ergonomic aliases over the same native session registry
and dispatcher. Use them for common app flows; use `nativeCall` for the complete
surface and future upstream commands.

An agent should treat the native library as a stateful tool server. It should
not infer results from filenames or reimplement HBC parsing in the model.

## Agent contract

### Initialization

For a Kotlin host, the common direct API is:

```kotlin
val handle = HermesNative.openHermesFile(path)
try {
    val metadata = JSONObject(HermesNative.getMetadata(handle))
    val functions = JSONObject(HermesNative.listFunctions(handle))
    val code = JSONObject(HermesNative.decompileFunction(handle, 42))
} finally {
    HermesNative.closeHermesFile(handle)
}
```

`generateFridaHooks` and `patchFunction` are also first-class direct methods.
All direct results use the same `{ok,result}` / `{ok:false,error}` envelope.

1. Confirm the ABI library is packaged and loaded.
2. Copy the input bundle to an app-private filesystem path if it came from an
   Android asset or content URI.
3. Open exactly one session for the input path.
4. Check that the returned handle is positive.
5. Close the handle in a `finally`/`use` block.

### Request format

Each request contains:

```json
{
  "operation": "info",
  "arguments": {}
}
```

The Android host converts it to:

```kotlin
HermesNative.nativeCall(handle, operation, JSONObject(arguments).toString())
```

### Success format

```json
{
  "ok": true,
  "result": "..."
}
```

The result may be a string, object, array, or number depending on the command.

### Error format

```json
{
  "ok": false,
  "error": {
    "code": "DECOMPILER_ERROR",
    "message": "..."
  }
}
```

An agent must stop or adapt when `ok` is false. It must not ask a user to
interpret an error as if it were a decompiler result.

## Tool manifest for an AI host

An Android host can expose the following logical tools to an agent. The host
should keep the native `operation` names unchanged to make logs and prompts
portable.

| Tool | Required arguments | Optional arguments | Result |
|---|---|---|---|
| `hermes_info` | none | none | File metadata object |
| `hermes_list_functions` | none | `limit` | Object containing `total` and `functions` array |
| `hermes_disassemble` | `function_id` | `show_offsets` | Text |
| `hermes_decompile_function` | `function_id` | `show_offsets`, `propagate`, `simplify`, `recover_structures`, `assembly`, `resolve_closures` | JavaScript-like text |
| `hermes_decompile_full` | `function_id` | none | Full-pipeline function text |
| `hermes_decompile_all` | none | none | Large text |
| `hermes_modules` | none | `limit` | Object containing `total` and module metadata array |
| `hermes_dependencies` | `module_id` | `depth` | Dependency tree text |
| `hermes_xref` | `query` | `kind` | Object containing query, kind, and match array |
| `hermes_cfg` | `function_id` | none | Graphviz DOT |
| `hermes_callgraph` | none | `function_id`, `depth`, `dot` | Text or DOT |
| `hermes_closures` | `function_id` | none | Object containing `functionId` and `slots` array |
| `hermes_dump` | none | `kind` | Text |
| `hermes_dump_table` | `kind` | `json` | Text or JSON |
| `hermes_secret_scan` | none | `show_full` | Redacted report |
| `hermes_emit_hasm` | `function_id` | none | HASM text |
| `hermes_patch_string` | `new_value`, `output_path` and either `id` or `old_value` | none | Output path |
| `hermes_patch_function` | `function_id`, `hasm`, `output_path` | none | Output path |
| `hermes_inject_stub` | `function_id`, `output_path` | `kind` (`nop`/`log`) | Output path |
| `hermes_create` | `output_path` | `version`, `strings` | Output path and version |
| `hermes_frida_hooks` | `module_id`, `output_dir` | `exports` | Output directory |
| `hermes_versions` | none | none | Supported versions |

The host can implement these as MCP tools, function-calling tools, REST-like
commands, or internal Kotlin methods. The native protocol remains the source
of truth.

## Recommended agent workflow

### Workflow A: identify the input

Use this sequence before interpreting code:

```text
1. nativeOpen(inputPath)
2. info {}
3. list-functions {"limit": 100}
4. list-versions {}
```

The `info` result gives the HBC version, function count, string count, and global
function index. Record these facts in the agent's working state. The parser can
fall back to the nearest older opcode table when an exact table is unavailable;
record the manifest's upstream commit alongside the observed HBC version.

Example result:

```json
{
  "ok": true,
  "result": {
    "path": "/data/user/0/example/files/index.android.bundle.hbc",
    "version": 98,
    "functions": 3200,
    "strings": 18000,
    "globalFunction": 0
  }
}
```

### Workflow B: inspect a suspicious function

```text
1. disasm {"function_id": 42, "show_offsets": true}
2. decompile {"function_id": 42, "resolve_closures": true}
3. closures {"function_id": 42}
4. graphviz {"function_id": 42}
5. xref {"query": "endpoint", "kind": "string"}
```

Use disassembly when the high-level output appears ambiguous. Use Graphviz only
when a human or downstream graph renderer can consume DOT.

### Workflow C: understand a React Native bundle

```text
1. info {}
2. modules {"limit": 500}
3. deps {"module_id": 12, "depth": 5}
4. decompile-full {"function_id": <module factory function id>}
5. callgraph {"function_id": <function id>, "depth": 5, "dot": false}
```

A Metro `module_id` is not necessarily the same as a function ID. The `modules`
result provides both values; preserve them separately in agent state.

### Workflow D: search for sensitive strings

```text
1. dump {"kind": "strings"}
2. xref {"query": "api", "kind": "string"}
3. secrets {"show_full": false}
```

Keep secret results redacted. Only request `show_full=true` after explicit user
authorization and only if the host has a secure result-handling policy.

### Workflow E: patch safely

```text
1. Preserve the original input.
2. emit-hasm {"function_id": 42} if editing a function.
3. Ask for or generate a minimal, reviewed change.
4. patch-string / patch-function / inject-stub with a new output_path.
5. Close the original session.
6. Open the output file in a new session.
7. Run info and a targeted disasm/decompile to validate the output.
```

Never overwrite the original bundle by default. The bridge writes a new output
file, while the existing session continues to represent the original input.

## Operation schemas and examples

### `info`

Arguments:

```json
{}
```

Use it as the first command after opening a session.

### `list-functions`

Arguments:

```json
{"limit": 100}
```

`limit` is optional. The result is an object with `total` and `functions`.
Returned function IDs are zero-based and include `name`, `params`, `frame`,
`size`, and `offset`.

### `disasm` / `disassembleFunction`

Arguments:

```json
{
  "function_id": 42,
  "show_offsets": true
}
```

`function_id` is required. `show_offsets` defaults to true.

### `decompile` / `decompileFunction`

Arguments:

```json
{
  "function_id": 42,
  "show_offsets": false,
  "propagate": true,
  "simplify": true,
  "recover_structures": true,
  "assembly": false,
  "resolve_closures": false
}
```

The operation is function-scoped and usually cheaper than full-pipeline calls.

### `decompile-full` / `decompileFunctionFull`

Arguments:

```json
{"function_id": 42}
```

This builds the full pipeline lazily and can take substantially longer on a
large bundle.

### `decompile-all`

Arguments:

```json
{}
```

This can return a very large response. Agents should prefer targeted functions
or modules unless the user explicitly asks for the entire output.

### `modules` / `list-modules`

The result is an object with `total` and `modules`; each module contains an
export array rather than a bare module array.

Arguments:

```json
{"limit": 200}
```

This may trigger full-pipeline analysis. Each module includes `id`,
`functionId`, `name`, `dependencies`, and an export list containing export names
and function IDs.

### `deps` / `module-deps`

Arguments:

```json
{
  "module_id": 12,
  "depth": 5
}
```

`depth` defaults to 8. Keep it bounded to prevent noisy output.

### `xref` / `search-strings` / `search-functions`

String search:

```json
{"query": "login", "kind": "string"}
```

Function search:

```json
{"query": "42", "kind": "function"}
```

The function-search aliases route through the same JSON shape; set `kind` to
`function` explicitly.

### `graph` / `graphviz` / `getControlFlowGraph`

Arguments:

```json
{"function_id": 42}
```

Returns Graphviz DOT for one function's CFG.

### `callgraph`

Arguments:

```json
{
  "function_id": 42,
  "depth": 5,
  "dot": false
}
```

`function_id` is optional. Without a root, the operation renders the bundle
call graph. Prefer a root and bounded depth for agent responses.

### `dump`

Arguments:

```json
{"kind": "functions"}
```

Supported kinds are `functions`, `strings` (default), and `all`.

### `dump-table` / `dumpTable`

Arguments:

```json
{
  "kind": "sections",
  "json": true
}
```

Supported kinds are `cjs-modules`, `regexp`, `obj-shapes`, `function-sources`,
`string-kinds`, `sections`, `big-int`, and `array-buffer`.

### `closures`

Arguments:

```json
{"function_id": 42}
```

Returns an object containing `functionId` and captured `slots`. Treat slot
names and inferred values as analysis hints, not proof of source-level
semantics.

### `secrets` / `scanSecrets`

Arguments:

```json
{"show_full": false}
```

The default is redacted output. Never place unredacted results in prompts,
telemetry, crash reports, or release notes without authorization.

### `emit-hasm`

Arguments:

```json
{"function_id": 42}
```

Use the returned text as the starting point for a reviewed bytecode edit.

### `patch-string`

By string ID:

```json
{
  "id": 17,
  "new_value": "replacement",
  "output_path": "/data/user/0/app/files/patched.hbc"
}
```

By old value:

```json
{
  "old_value": "https://old.example",
  "new_value": "https://new.example",
  "output_path": "/data/user/0/app/files/patched.hbc"
}
```

### `patch-function`

Arguments:

```json
{
  "function_id": 42,
  "hasm": "Ret r0",
  "output_path": "/data/user/0/app/files/patched.hbc"
}
```

A real patch should normally be based on `emit-hasm`; validate operands and
control-flow labels before calling.

### `inject-stub`

Arguments:

```json
{
  "function_id": 42,
  "kind": "nop",
  "output_path": "/data/user/0/app/files/injected.hbc"
}
```

`kind` is `nop` by default or `log` for an entry logging stub. The `log` path
may require bytecode/runtime support present in the target version.

### `create`

Arguments:

```json
{
  "version": 98,
  "strings": ["global", "hello"],
  "output_path": "/data/user/0/app/files/minimal.hbc"
}
```

This creates a minimal valid HBC image; it does not compile JavaScript.

### `frida-hooks` / `generateFridaHooks`

Arguments:

```json
{
  "module_id": 12,
  "exports": "login,default",
  "output_dir": "/data/user/0/app/files/hooks"
}
```

The output directory receives `before.js`, `after.js`, `agent.js`, and `run.sh`.
Treat generated hooks as code and review them before use.

### `list-versions`

Arguments:

```json
{}
```

Returns the supported version range and generated available-version table.

## Agent policy recommendations

An AI host should add these policies around the native calls:

### Authorization

Require the user to confirm that they own or are authorized to analyze the app.
Do not provide workflows intended to steal credentials, bypass access controls,
or tamper with third-party apps without authorization.

### Input validation

- Accept only expected operation names.
- Validate integer IDs are non-negative and bounded.
- Bound `limit` and graph `depth`.
- Constrain output paths to an app-controlled directory.
- Reject path traversal and accidental overwrite of the input.
- Limit maximum output size before moving data into a model context.

### Model-context hygiene

- Summarize large disassemblies instead of injecting all text into the prompt.
- Retrieve targeted functions after identifying relevant IDs.
- Keep raw HBC paths and output paths out of user-visible text when sensitive.
- Redact secrets by default.
- Preserve the distinction between evidence and inference.

### Failure handling

- `INVALID_HANDLE`: reopen the input and retry once.
- `INVALID_JSON`: regenerate arguments from a typed schema.
- `INVALID_ARGUMENT`: correct the exact field or operation.
- `DECOMPILER_ERROR`: report the HBC version/function/module context; do not
  silently substitute an unrelated result.
- `WRITE_ERROR` or `IO_ERROR`: preserve the original and report the output path.

### Performance

- Use `info` and `list-functions` before expensive analysis.
- Prefer `decompile` for one function.
- Use `decompile-full` only when IPA, closures, or ESM/module quality is needed.
- Bound dependency and call-graph depth.
- Run all native calls on a worker thread.
- Treat `decompile-all` as an explicit large-output operation.

## Host-side Kotlin pseudocode

```kotlin
suspend fun runAgentTool(
    hbcPath: String,
    operation: String,
    arguments: JSONObject,
): JSONObject = withContext(Dispatchers.Default) {
    HermesSession(hbcPath).use { session ->
        session.open()
        val response = session.call(operation, arguments)
        if (!response.optBoolean("ok", false)) {
            throw IllegalStateException(response.optJSONObject("error").toString())
        }
        response
    }
}
```

For multi-step investigations, keep one session open while querying the same
input. Reopen only after a write operation or when switching input files.

## Scope and limitations

- Results depend on the HBC version, bytecode layout, debug information, and
  the upstream engine version recorded in the release manifest.
- Decompiled JavaScript is reconstructed output, not guaranteed original source.
- XRef function matching is analysis-based and should be verified in context.
- Full-pipeline analysis is computationally expensive.
- JNI returns text as a Java `String`; very large results can create memory
  pressure. Add host-side size limits and prefer targeted operations.
- Write support edits HBC/HASM and does not recompile JavaScript.
- The current session remains attached to the original input after a write.

## Provenance

Every automated Android release is built from the upstream `main` commit detected
by the daily GitHub Actions poll. Use `manifest.json` and `SHA256SUMS` to record
which engine revision was used in an investigation or agent report.
