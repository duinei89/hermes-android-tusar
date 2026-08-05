use jni::{
    objects::{JClass, JString},
    sys::jstring,
    JNIEnv,
};

fn make_string(mut env: JNIEnv, text: String) -> jstring {
    match env.new_string(text) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Kotlin/Java class:
/// com.tusar.hermes.HermesNative
///
/// Kotlin method:
/// external fun nativeVersion(): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeVersion(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    make_string(
        env,
        "libtusar.so loaded successfully".to_string(),
    )
}

/// Kotlin method:
/// external fun nativeOpen(path: String): Long
///
/// This is a placeholder session function.
/// The actual hbc-decomp parser will be connected here later.
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeOpen(
    mut env: JNIEnv,
    _class: JClass,
    path: JString,
) -> i64 {
    let path: String = match env.get_string(&path) {
        Ok(value) => value.into(),
        Err(_) => return 0,
    };

    if path.is_empty() {
        return 0;
    }

    // TODO:
    // Open and parse the Hermes bytecode using hbc-decomp.
    //
    // Do not use a Rust pointer directly as a handle.
    // Later, use a thread-safe session registry.

    0
}

/// Kotlin method:
/// external fun nativeClose(handle: Long)
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeClose(
    _env: JNIEnv,
    _class: JClass,
    _handle: i64,
) {
    // TODO: Remove the analysis session from the Rust registry.
}

/// Kotlin method:
/// external fun nativeCall(
///     handle: Long,
///     operation: String,
///     argumentsJson: String
/// ): String
#[no_mangle]
pub extern "system" fn Java_com_tusar_hermes_HermesNative_nativeCall(
    mut env: JNIEnv,
    _class: JClass,
    _handle: i64,
    operation: JString,
    arguments_json: JString,
) -> jstring {
    let operation: String = match env.get_string(&operation) {
        Ok(value) => value.into(),
        Err(_) => {
            return make_string(
                env,
                r#"{"ok":false,"error":"Invalid operation string"}"#.to_string(),
            )
        }
    };

    let arguments_json: String = match env.get_string(&arguments_json) {
        Ok(value) => value.into(),
        Err(_) => {
            return make_string(
                env,
                r#"{"ok":false,"error":"Invalid arguments JSON"}"#.to_string(),
            )
        }
    };

    let response = serde_json::json!({
        "ok": false,
        "error": "Operation is not implemented yet",
        "operation": operation,
        "arguments": arguments_json
    });

    make_string(env, response.to_string())
}
