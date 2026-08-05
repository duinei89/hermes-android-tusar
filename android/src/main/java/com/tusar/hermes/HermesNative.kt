package com.tusar.hermes

import org.json.JSONArray
import org.json.JSONObject

/**
 * Direct Kotlin facade for libtusar.so.
 *
 * The native methods return JSON strings for consistency across the full
 * decompiler surface. Every operation except open/close returns either:
 * {"ok":true,"result":...} or
 * {"ok":false,"error":{"code":"...","message":"..."}}.
 *
 * Long-running operations must be called from a worker thread, never from the
 * Android main thread. Copy an APK asset or content URI to a real file first;
 * the native library accepts filesystem paths only.
 */
object HermesNative {
    init {
        System.loadLibrary("tusar")
    }

    @JvmStatic external fun nativeVersion(): String
    @JvmStatic external fun nativeOpen(path: String): Long
    @JvmStatic external fun nativeLastError(): String
    @JvmStatic external fun nativeClose(handle: Long)
    @JvmStatic external fun nativeCall(handle: Long, operation: String, argumentsJson: String): String

    // Direct JNI API.
    @JvmStatic external fun openHermesFile(path: String): Long
    @JvmStatic external fun getMetadata(handle: Long): String
    @JvmStatic external fun listFunctions(handle: Long): String
    @JvmStatic external fun disassembleFunction(handle: Long, functionId: Int): String
    @JvmStatic external fun decompileFunction(handle: Long, functionId: Int): String
    @JvmStatic external fun searchStrings(handle: Long, query: String): String
    @JvmStatic external fun searchFunctions(handle: Long, query: String): String
    @JvmStatic external fun getControlFlowGraph(handle: Long, functionId: Int): String
    @JvmStatic external fun scanSecrets(handle: Long): String
    @JvmStatic external fun generateFridaHooks(
        handle: Long,
        moduleId: Int,
        outputDir: String,
        exports: String = "",
    ): String
    @JvmStatic external fun patchFunction(
        handle: Long,
        functionId: Int,
        hasm: String,
        outputPath: String,
    ): String
    @JvmStatic external fun closeHermesFile(handle: Long)

    fun openOrThrow(path: String): Long {
        val handle = openHermesFile(path)
        check(handle > 0L) { nativeLastError().ifBlank { "Unable to open Hermes bytecode: $path" } }
        return handle
    }

    /** Generic access to every native command, including future upstream commands. */
    fun call(handle: Long, operation: String, arguments: JSONObject = JSONObject()): JSONObject {
        return JSONObject(nativeCall(handle, operation, arguments.toString()))
    }

    fun call(handle: Long, operation: String, arguments: Map<String, Any?>): JSONObject {
        val json = JSONObject()
        arguments.forEach { (key, value) -> json.put(key, value) }
        return call(handle, operation, json)
    }

    fun listVersions(): JSONObject = call(0L, "list-versions")

    /** Full-pipeline function decompilation. */
    fun decompileFunctionFull(handle: Long, functionId: Int): JSONObject =
        call(handle, "decompile-full", JSONObject().put("function_id", functionId))

    fun decompileAll(handle: Long): JSONObject = call(handle, "decompile-all")

    fun modules(handle: Long, limit: Int? = null): JSONObject =
        call(handle, "modules", JSONObject().apply { limit?.let { put("limit", it) } })

    fun moduleDependencies(handle: Long, moduleId: Int, depth: Int = 8): JSONObject =
        call(handle, "deps", JSONObject().put("module_id", moduleId).put("depth", depth))

    fun callGraph(handle: Long, functionId: Int? = null, depth: Int = 8, dot: Boolean = false): JSONObject =
        call(handle, "callgraph", JSONObject().apply {
            functionId?.let { put("function_id", it) }
            put("depth", depth)
            put("dot", dot)
        })

    fun dump(handle: Long, kind: String = "strings"): JSONObject =
        call(handle, "dump", JSONObject().put("kind", kind))

    fun dumpTable(handle: Long, kind: String, json: Boolean = false): JSONObject =
        call(handle, "dump-table", JSONObject().put("kind", kind).put("json", json))

    fun debugInfo(handle: Long): JSONObject = call(handle, "debug")

    fun extractModules(handle: Long, outputDir: String): JSONObject =
        call(handle, "extract", JSONObject().put("output_dir", outputDir))

    fun binaryDiff(handle: Long, otherPath: String): JSONObject =
        call(handle, "bin-diff", JSONObject().put("other_path", otherPath))

    fun emitHasm(handle: Long, functionId: Int): JSONObject =
        call(handle, "emit-hasm", JSONObject().put("function_id", functionId))

    fun assembleFunction(
        handle: Long,
        functionId: Int,
        hasm: String,
        outputPath: String,
        overwrite: Boolean = false,
    ): JSONObject = call(handle, "asm", JSONObject()
        .put("function_id", functionId)
        .put("hasm", hasm)
        .put("output_path", outputPath)
        .put("overwrite", overwrite))

    fun checkAssembly(handle: Long, functionId: Int): JSONObject =
        call(handle, "asm-check", JSONObject().put("function_id", functionId))

    fun patchStringById(
        handle: Long,
        id: Int,
        newValue: String,
        outputPath: String,
        overwrite: Boolean = false,
    ): JSONObject = call(handle, "patch-string", JSONObject()
        .put("id", id)
        .put("new_value", newValue)
        .put("output_path", outputPath)
        .put("overwrite", overwrite))

    fun patchString(handle: Long, oldValue: String, newValue: String, outputPath: String): JSONObject =
        call(handle, "patch-string", JSONObject()
            .put("old_value", oldValue)
            .put("new_value", newValue)
            .put("output_path", outputPath))

    fun injectStub(handle: Long, functionId: Int, kind: String, outputPath: String): JSONObject =
        call(handle, "inject-stub", JSONObject()
            .put("function_id", functionId)
            .put("kind", kind)
            .put("output_path", outputPath))

    fun createFile(outputPath: String, version: Int = 96, strings: List<String> = listOf("global")): JSONObject =
        call(0L, "create", JSONObject()
            .put("output_path", outputPath)
            .put("version", version)
            .put("strings", JSONArray().apply { strings.forEach(::put) }))
}

/**
 * Owns one native session and closes it reliably with Kotlin `use`.
 * The same instance should not be used concurrently from multiple coroutines.
 */
class HermesSession(val path: String) : AutoCloseable {
    var handle: Long = 0L
        private set

    val isOpen: Boolean
        get() = handle > 0L

    fun open(): HermesSession {
        check(!isOpen) { "Hermes session is already open" }
        handle = HermesNative.openOrThrow(path)
        return this
    }

    fun requireOpen() {
        check(isOpen) { "Hermes session is not open" }
    }

    fun call(operation: String, arguments: JSONObject = JSONObject()): JSONObject {
        requireOpen()
        return HermesNative.call(handle, operation, arguments)
    }

    fun metadata(): JSONObject = JSONObject(HermesNative.getMetadata(requireHandle()))
    fun functions(): JSONObject = JSONObject(HermesNative.listFunctions(requireHandle()))
    fun disassemble(functionId: Int): JSONObject = JSONObject(
        HermesNative.disassembleFunction(requireHandle(), functionId),
    )
    fun decompile(functionId: Int): JSONObject = JSONObject(
        HermesNative.decompileFunction(requireHandle(), functionId),
    )
    fun strings(query: String): JSONObject = JSONObject(
        HermesNative.searchStrings(requireHandle(), query),
    )
    fun functionReferences(query: String): JSONObject = JSONObject(
        HermesNative.searchFunctions(requireHandle(), query),
    )
    fun controlFlowGraph(functionId: Int): JSONObject = JSONObject(
        HermesNative.getControlFlowGraph(requireHandle(), functionId),
    )
    fun secrets(): JSONObject = JSONObject(HermesNative.scanSecrets(requireHandle()))

    // Complete Android-facing command surface. These methods call the same
    // native dispatcher as the direct JNI aliases and keep command JSON out of
    // application code.
    fun decompileFull(functionId: Int): JSONObject =
        HermesNative.decompileFunctionFull(requireHandle(), functionId)

    fun decompileAll(): JSONObject = HermesNative.decompileAll(requireHandle())
    fun modules(limit: Int? = null): JSONObject = HermesNative.modules(requireHandle(), limit)
    fun dependencies(moduleId: Int, depth: Int = 8): JSONObject =
        HermesNative.moduleDependencies(requireHandle(), moduleId, depth)
    fun crossReferences(query: String, kind: String = "string"): JSONObject =
        call("xref", JSONObject().put("query", query).put("kind", kind))
    fun callGraph(functionId: Int? = null, depth: Int = 8, dot: Boolean = false): JSONObject =
        HermesNative.callGraph(requireHandle(), functionId, depth, dot)
    fun closures(functionId: Int): JSONObject =
        call("closures", JSONObject().put("function_id", functionId))
    fun debug(): JSONObject = HermesNative.debugInfo(requireHandle())
    fun dump(kind: String = "strings"): JSONObject = HermesNative.dump(requireHandle(), kind)
    fun dumpTable(kind: String, json: Boolean = false): JSONObject =
        HermesNative.dumpTable(requireHandle(), kind, json)
    fun extract(outputDir: String): JSONObject =
        HermesNative.extractModules(requireHandle(), outputDir)
    fun binaryDiff(otherPath: String): JSONObject =
        HermesNative.binaryDiff(requireHandle(), otherPath)
    fun emitHasm(functionId: Int): JSONObject =
        HermesNative.emitHasm(requireHandle(), functionId)
    fun assemble(
        functionId: Int,
        hasm: String,
        outputPath: String,
        overwrite: Boolean = false,
    ): JSONObject = HermesNative.assembleFunction(
        requireHandle(),
        functionId,
        hasm,
        outputPath,
        overwrite,
    )

    fun assembleFunctions(
        edits: List<Pair<Int, String>>,
        outputPath: String,
        overwrite: Boolean = false,
    ): JSONObject {
        val rows = JSONArray()
        edits.forEach { (functionId, hasm) ->
            rows.put(JSONObject().put("function_id", functionId).put("hasm", hasm))
        }
        return call(
            requireHandle(),
            "patch-functions",
            JSONObject().put("edits", rows).put("output_path", outputPath).put("overwrite", overwrite),
        )
    }
    fun assemblyCheck(functionId: Int): JSONObject =
        HermesNative.checkAssembly(requireHandle(), functionId)
    fun patchString(id: Int, newValue: String, outputPath: String, overwrite: Boolean = false): JSONObject =
        call("patch-string", JSONObject()
            .put("id", id)
            .put("new_value", newValue)
            .put("output_path", outputPath)
            .put("overwrite", overwrite))
    fun patchString(oldValue: String, newValue: String, outputPath: String): JSONObject =
        HermesNative.patchString(requireHandle(), oldValue, newValue, outputPath)
    fun injectStub(functionId: Int, kind: String = "nop", outputPath: String): JSONObject =
        HermesNative.injectStub(requireHandle(), functionId, kind, outputPath)
    fun fridaHooks(moduleId: Int, outputDir: String, exports: String = ""): JSONObject =
        JSONObject(HermesNative.generateFridaHooks(requireHandle(), moduleId, outputDir, exports))

    override fun close() {
        if (isOpen) {
            HermesNative.closeHermesFile(handle)
            handle = 0L
        }
    }

    private fun requireHandle(): Long {
        requireOpen()
        return handle
    }
}
