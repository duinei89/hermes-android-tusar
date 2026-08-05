mod commands;
mod session;

use hbc_decomp::{BytecodeFile, BytecodeFormat};
use jni::{
    objects::{JClass, JString},
    sys::jstring,
    JNIEnv,
};
use serde_json::Value;
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
    serde_json::json!({
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
///
/// The path must point to a readable Hermes bytecode file. A positive opaque
/// session handle is returned on success; zero means the file could not be
/// opened or parsed.
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeOpen(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> i64 {
    let path: String = match read_jstring(&mut env, &path, "path") {
        Ok(path) if !path.is_empty() => path,
        _ => return 0,
    };
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
    handle as i64
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

    let response = if handle <= 0 {
        commands::dispatch(None, &operation, arguments)
    } else if let Some(session) = store().get(handle as u64) {
        let mut session = session.write();
        commands::dispatch(Some(&mut session), &operation, arguments)
    } else {
        serde_json::json!({
            "ok": false,
            "error": {"code": "INVALID_HANDLE", "message": "Unknown session handle"}
        })
    };

    make_string(env, response.to_string())
}
