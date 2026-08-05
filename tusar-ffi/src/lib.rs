mod commands;
mod session;

use hbc_decomp::{BytecodeFile, BytecodeFormat};
use jni::{
    objects::{JClass, JString},
    sys::jstring,
    JNIEnv,
};
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::session::{Session, SessionStore};

static STORE: OnceLock<SessionStore> = OnceLock::new();

fn store() -> &'static SessionStore {
    STORE.get_or_init(SessionStore::new)
}

fn make_string(mut env: JNIEnv, text: String) -> jstring {
    match env.new_string(text) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn json_error(code: &str, message: impl Into<String>) -> String {
    json!({
        "ok": false,
        "error": {"code": code, "message": message.into()}
    })
    .to_string()
}

fn read_jstring(env: &mut JNIEnv, value: &JString, name: &str) -> Result<String, String> {
    env.get_string(value)
        .map(|value| value.into())
        .map_err(|_| json_error("INVALID_ARGUMENT", format!("Invalid {name} string")))
}

fn open_path(path: String) -> i64 {
    if path.is_empty() {
        return 0;
    }

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return 0,
    };
    let file = match BytecodeFile::parse_auto(&bytes) {
        Ok(file) => file,
        Err(_) => return 0,
    };
    let format = match BytecodeFormat::for_version_or_latest(file.header.version) {
        Ok((format, _)) => format,
        Err(_) => return 0,
    };

    let handle = store().insert(Session {
        input_path: path,
        bytes,
        file,
        format,
        pipeline_ctx: None,
    });
    i64::try_from(handle).unwrap_or(0)
}

fn dispatch_command(handle: i64, operation: &str, arguments: Value) -> Value {
    if handle <= 0 {
        return commands::dispatch(None, operation, arguments);
    }

    match store().get(handle as u64) {
        Some(session) => {
            let mut session = session.write();
            commands::dispatch(Some(&mut session), operation, arguments)
        }
        None => json!({
            "ok": false,
            "error": {"code": "INVALID_HANDLE", "message": "Unknown session handle"}
        }),
    }
}

fn call_json(handle: i64, operation: &str, arguments: Value) -> String {
    dispatch_command(handle, operation, arguments).to_string()
}

/// Kotlin/Java class: com.tusar.hermes.HermesNative
///
/// Kotlin method: external fun nativeVersion(): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeVersion(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    make_string(env, "libtusar.so loaded successfully".to_string())
}

/// Kotlin method: external fun nativeOpen(path: String): Long
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeOpen(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> i64 {
    let path = match read_jstring(&mut env, &path, "path") {
        Ok(path) => path,
        Err(_) => return 0,
    };
    open_path(path)
}

/// Kotlin method: external fun nativeClose(handle: Long)
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeClose(
    _env: JNIEnv,
    _class: JClass,
    handle: i64,
) {
    if handle > 0 {
        store().remove(handle as u64);
    }
}

/// Kotlin method:
/// external fun nativeCall(handle: Long, operation: String, argumentsJson: String): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeCall(
    mut env: JNIEnv,
    _class: JClass,
    handle: i64,
    operation: JString,
    arguments_json: JString,
) -> jstring {
    let operation = match read_jstring(&mut env, &operation, "operation") {
        Ok(value) => value,
        Err(error) => return make_string(env, error),
    };
    let arguments_json = match read_jstring(&mut env, &arguments_json, "argumentsJson") {
        Ok(value) => value,
        Err(error) => return make_string(env, error),
    };
    let arguments: Value = match serde_json::from_str(&arguments_json) {
        Ok(value) => value,
        Err(error) => {
            return make_string(
                env,
                json_error("INVALID_JSON", format!("Invalid arguments JSON: {error}")),
            )
        }
    };

    make_string(env, call_json(handle, &operation, arguments))
}

/// Kotlin method: external fun openHermesFile(path: String): Long
///
/// Opens and parses a Hermes bytecode file. The returned positive value is an
/// opaque handle owned by the native session registry; zero means open/parse
/// failed. Always pair a successful call with closeHermesFile.
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_openHermesFile(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> i64 {
    let path = match read_jstring(&mut env, &path, "path") {
        Ok(path) => path,
        Err(_) => return 0,
    };
    open_path(path)
}

/// Kotlin method: external fun getMetadata(handle: Long): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_getMetadata(
    env: JNIEnv,
    _class: JClass,
    handle: i64,
) -> jstring {
    make_string(env, call_json(handle, "info", json!({})))
}

/// Kotlin method: external fun listFunctions(handle: Long): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_listFunctions(
    env: JNIEnv,
    _class: JClass,
    handle: i64,
) -> jstring {
    make_string(env, call_json(handle, "list-functions", json!({})))
}

/// Kotlin method: external fun disassembleFunction(handle: Long, functionId: Int): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_disassembleFunction(
    env: JNIEnv,
    _class: JClass,
    handle: i64,
    function_id: i32,
) -> jstring {
    if function_id < 0 {
        return make_string(env, json_error("INVALID_ARGUMENT", "functionId must be non-negative"));
    }
    make_string(
        env,
        call_json(handle, "disasm", json!({"function_id": function_id as u32})),
    )
}

/// Kotlin method: external fun decompileFunction(handle: Long, functionId: Int): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_decompileFunction(
    env: JNIEnv,
    _class: JClass,
    handle: i64,
    function_id: i32,
) -> jstring {
    if function_id < 0 {
        return make_string(env, json_error("INVALID_ARGUMENT", "functionId must be non-negative"));
    }
    make_string(
        env,
        call_json(handle, "decompile", json!({"function_id": function_id as u32})),
    )
}

/// Kotlin method: external fun searchStrings(handle: Long, query: String): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_searchStrings(
    mut env: JNIEnv,
    _class: JClass,
    handle: i64,
    query: JString,
) -> jstring {
    let query = match read_jstring(&mut env, &query, "query") {
        Ok(query) => query,
        Err(error) => return make_string(env, error),
    };
    make_string(
        env,
        call_json(handle, "search-strings", json!({"query": query, "kind": "string"})),
    )
}

/// Kotlin method: external fun searchFunctions(handle: Long, query: String): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_searchFunctions(
    mut env: JNIEnv,
    _class: JClass,
    handle: i64,
    query: JString,
) -> jstring {
    let query = match read_jstring(&mut env, &query, "query") {
        Ok(query) => query,
        Err(error) => return make_string(env, error),
    };
    make_string(
        env,
        call_json(handle, "search-functions", json!({"query": query, "kind": "function"})),
    )
}

/// Kotlin method: external fun getControlFlowGraph(handle: Long, functionId: Int): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_getControlFlowGraph(
    env: JNIEnv,
    _class: JClass,
    handle: i64,
    function_id: i32,
) -> jstring {
    if function_id < 0 {
        return make_string(env, json_error("INVALID_ARGUMENT", "functionId must be non-negative"));
    }
    make_string(
        env,
        call_json(handle, "graphviz", json!({"function_id": function_id as u32})),
    )
}

/// Kotlin method: external fun scanSecrets(handle: Long): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_scanSecrets(
    env: JNIEnv,
    _class: JClass,
    handle: i64,
) -> jstring {
    make_string(env, call_json(handle, "secrets", json!({})))
}

/// Kotlin method:
/// external fun generateFridaHooks(handle: Long, moduleId: Int, outputDir: String, exports: String): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_generateFridaHooks(
    mut env: JNIEnv,
    _class: JClass,
    handle: i64,
    module_id: i32,
    output_dir: JString,
    exports: JString,
) -> jstring {
    if module_id < 0 {
        return make_string(env, json_error("INVALID_ARGUMENT", "moduleId must be non-negative"));
    }
    let output_dir = match read_jstring(&mut env, &output_dir, "outputDir") {
        Ok(value) => value,
        Err(error) => return make_string(env, error),
    };
    let exports = match read_jstring(&mut env, &exports, "exports") {
        Ok(value) => value,
        Err(error) => return make_string(env, error),
    };
    make_string(
        env,
        call_json(
            handle,
            "frida-hooks",
            json!({
                "module_id": module_id as u32,
                "output_dir": output_dir,
                "exports": exports,
            }),
        ),
    )
}

/// Kotlin method:
/// external fun patchFunction(handle: Long, functionId: Int, hasm: String, outputPath: String): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_patchFunction(
    mut env: JNIEnv,
    _class: JClass,
    handle: i64,
    function_id: i32,
    hasm: JString,
    output_path: JString,
) -> jstring {
    if function_id < 0 {
        return make_string(env, json_error("INVALID_ARGUMENT", "functionId must be non-negative"));
    }
    let hasm = match read_jstring(&mut env, &hasm, "hasm") {
        Ok(value) => value,
        Err(error) => return make_string(env, error),
    };
    let output_path = match read_jstring(&mut env, &output_path, "outputPath") {
        Ok(value) => value,
        Err(error) => return make_string(env, error),
    };
    make_string(
        env,
        call_json(
            handle,
            "patch-function",
            json!({
                "function_id": function_id as u32,
                "hasm": hasm,
                "output_path": output_path,
            }),
        ),
    )
}

/// Kotlin method: external fun closeHermesFile(handle: Long)
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_closeHermesFile(
    _env: JNIEnv,
    _class: JClass,
    handle: i64,
) {
    if handle > 0 {
        store().remove(handle as u64);
    }
}
