use hbc_decomp::{BytecodeFile, DecompileOptionsV2, DisasmOptions};
use serde_json::{json, Value};
use std::path::Path;

use crate::session::Session;

fn ok(result: impl Into<Value>) -> Value {
    json!({"ok": true, "result": result.into()})
}

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({
        "ok": false,
        "error": {"code": code, "message": message.into()}
    })
}

fn required_string(args: &Value, name: &str) -> Result<String, Value> {
    args.get(name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| error("INVALID_ARGUMENT", format!("Missing string argument: {name}")))
}

fn required_u32(args: &Value, name: &str) -> Result<u32, Value> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| error("INVALID_ARGUMENT", format!("Missing integer argument: {name}")))
}

fn optional_u32(args: &Value, name: &str) -> Result<Option<u32>, Value> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| error("INVALID_ARGUMENT", format!("Invalid integer argument: {name}"))),
    }
}

fn string_arg(args: &Value, name: &str, default: &str) -> String {
    args.get(name)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn bool_arg(args: &Value, name: &str, default: bool) -> bool {
    args.get(name).and_then(Value::as_bool).unwrap_or(default)
}

fn session_error(result: hbc_decomp::Result<()>) -> Result<(), Value> {
    result.map_err(|e| error("DECOMPILER_ERROR", e.to_string()))
}

fn function_name(file: &BytecodeFile, id: usize) -> String {
    file.function_headers
        .get(id)
        .and_then(|header| file.string_at(header.function_name()))
        .map(|entry| entry.value.clone())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("f{id}"))
}

fn file_info(session: &Session) -> Value {
    let file = &session.file;
    ok(json!({
        "path": session.input_path,
        "version": file.header.version,
        "functions": file.header.function_count,
        "strings": file.header.string_count,
        "globalFunction": file.header.global_code_index,
    }))
}

fn list_functions(session: &Session, args: &Value) -> Value {
    let file = &session.file;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(file.function_headers.len())
        .min(file.function_headers.len());

    let functions: Vec<Value> = file
        .function_headers
        .iter()
        .enumerate()
        .take(limit)
        .map(|(id, header)| {
            json!({
                "id": id,
                "name": function_name(file, id),
                "params": header.param_count(),
                "frame": header.frame_size(),
                "size": header.bytecode_size_in_bytes(),
                "offset": header.offset(),
            })
        })
        .collect();
    ok(json!({"total": file.function_headers.len(), "functions": functions}))
}

fn decompile_function(session: &Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let options = DecompileOptionsV2 {
        resolve_strings: true,
        include_offsets: bool_arg(args, "show_offsets", false),
        propagate: bool_arg(args, "propagate", true),
        simplify: bool_arg(args, "simplify", true),
        recover_structures: bool_arg(args, "recover_structures", true),
        assembly_mode: bool_arg(args, "assembly", false),
    };
    let output = if bool_arg(args, "resolve_closures", false) {
        let closure_context = hbc_decomp::build_closure_context(&session.file, &session.format)
            .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
        hbc_decomp::decompile_function_v2_with_context(
            &session.file,
            &session.format,
            function_id,
            &options,
            Some(&closure_context),
        )
    } else {
        hbc_decomp::decompile_function_v2(
            &session.file,
            &session.format,
            function_id,
            &options,
        )
    }
    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    Ok(ok(output))
}

fn full_decompile_function(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    session_error(session.ensure_pipeline())?;
    let pipeline = session
        .pipeline_ctx
        .as_ref()
        .ok_or_else(|| error("DECOMPILER_ERROR", "Pipeline was not initialized"))?;
    Ok(ok(pipeline.generate_function_code(&session.file, function_id)))
}

fn decompile_all(session: &mut Session) -> Result<Value, Value> {
    session_error(session.ensure_pipeline())?;
    let pipeline = session
        .pipeline_ctx
        .as_ref()
        .ok_or_else(|| error("DECOMPILER_ERROR", "Pipeline was not initialized"))?;
    let mut output = String::new();
    for id in pipeline.all_ir.keys().copied() {
        output.push_str(&pipeline.generate_function_code(&session.file, id));
        output.push('\n');
    }
    Ok(ok(output))
}

fn disassemble(session: &Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let output = hbc_decomp::disassemble_function(
        &session.file,
        &session.format,
        function_id,
        &DisasmOptions {
            show_offsets: bool_arg(args, "show_offsets", true),
            show_labels: true,
            resolve_strings: true,
            enable_color: false,
        },
    )
    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    Ok(ok(output))
}

fn xrefs(session: &Session, args: &Value) -> Result<Value, Value> {
    let query = required_string(args, "query")?;
    let kind = string_arg(args, "kind", "string");
    let refs = if kind == "function" {
        let function_id = query
            .parse::<u32>()
            .map_err(|_| error("INVALID_ARGUMENT", "Function xref query must be an integer"))?;
        hbc_decomp::analysis::find_function_refs(&session.file, &session.format, function_id)
    } else {
        hbc_decomp::analysis::find_string_xrefs(&session.file, &session.format, &query)
    };
    let results: Vec<Value> = refs
        .into_iter()
        .map(|xref| {
            json!({
                "functionId": xref.function_id,
                "functionName": function_name(&session.file, xref.function_id as usize),
                "offset": xref.offset,
                "opcode": xref.opcode,
            })
        })
        .collect();
    Ok(ok(json!({"query": query, "kind": kind, "matches": results})))
}

fn modules(session: &mut Session, args: &Value) -> Result<Value, Value> {
    session_error(session.ensure_pipeline())?;
    let pipeline = session
        .pipeline_ctx
        .as_ref()
        .ok_or_else(|| error("DECOMPILER_ERROR", "Pipeline was not initialized"))?;
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(usize::MAX);
    let modules: Vec<Value> = pipeline
        .registry
        .modules
        .values()
        .take(limit)
        .map(|module| {
            let exports = module
                .exports
                .iter()
                .map(|(name, function_id)| json!({"name": name, "functionId": function_id}))
                .collect::<Vec<_>>();
            json!({
                "id": module.module_id,
                "functionId": module.function_id,
                "name": module.name,
                "dependencies": module.dependencies,
                "exports": exports,
            })
        })
        .collect();
    Ok(ok(json!({"total": pipeline.registry.modules.len(), "modules": modules})))
}

fn dependencies(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let module_id = required_u32(args, "module_id")?;
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(8);
    session_error(session.ensure_pipeline())?;
    let pipeline = session
        .pipeline_ctx
        .as_ref()
        .ok_or_else(|| error("DECOMPILER_ERROR", "Pipeline was not initialized"))?;
    let tree = pipeline.registry.get_dependency_tree(module_id, depth);
    Ok(ok(tree.format(0)))
}

fn dump(session: &Session, args: &Value) -> Value {
    let kind = string_arg(args, "kind", "strings");
    let file = &session.file;
    let mut output = String::new();
    match kind.as_str() {
        "functions" => {
            for (id, header) in file.function_headers.iter().enumerate() {
                output.push_str(&format!(
                    "Function {id}: name=\"{}\" params={} frame={} size={}\n",
                    function_name(file, id),
                    header.param_count(),
                    header.frame_size(),
                    header.bytecode_size_in_bytes()
                ));
            }
        }
        "all" => {
            output.push_str(&format!("=== {} strings ===\n", file.strings.len()));
            for (id, string) in file.strings.iter().enumerate() {
                output.push_str(&format!("{id}: {}\n", string.value));
            }
            output.push_str("\n=== functions ===\n");
            let functions = dump(session, &json!({"kind": "functions"}));
            if let Some(value) = functions.get("result") {
                output.push_str(value.as_str().unwrap_or_default());
            }
        }
        _ => {
            for (id, string) in file.strings.iter().enumerate() {
                output.push_str(&format!("{id}: {}\n", string.value));
            }
        }
    }
    ok(output)
}

fn dump_table(session: &Session, args: &Value) -> Result<Value, Value> {
    let kind = required_string(args, "kind")?;
    let table_kind = hbc_decomp::TableKind::parse(&kind)
        .ok_or_else(|| error("INVALID_ARGUMENT", format!("Unknown table kind: {kind}")))?;
    if bool_arg(args, "json", false) {
        Ok(ok(hbc_decomp::dump_table_json(&session.file, table_kind)))
    } else {
        Ok(ok(hbc_decomp::dump_table(&session.file, table_kind)))
    }
}

fn graphviz(session: &Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let options = hbc_decomp::IRBuilderOptions {
        resolve_strings: true,
        include_offsets: false,
        ..Default::default()
    };
    let mut builder = hbc_decomp::IRBuilder::new(&session.file, &session.format, options);
    let cfg = builder
        .build_function(function_id)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let name = function_name(&session.file, function_id as usize);
    Ok(ok(hbc_decomp::ir::generate_dot(&cfg, &name)))
}

fn callgraph(session: &Session, args: &Value) -> Result<Value, Value> {
    let root = optional_u32(args, "function_id")?;
    let depth = args
        .get("depth")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(8);
    let output = hbc_decomp::render_call_graph(
        &session.file,
        &session.format,
        root,
        depth,
        bool_arg(args, "dot", false),
    )
    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    Ok(ok(output))
}

fn closures(session: &Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let options = DecompileOptionsV2::optimized();
    let statements = hbc_decomp::generate_ir(
        &session.file,
        &session.format,
        function_id,
        &options,
        None,
        true,
    )
    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let info = hbc_decomp::ClosureInfo::analyze(&statements);
    let slots = info
        .slots
        .into_iter()
        .map(|(slot, value)| json!({"slot": slot, "value": format!("{value:?}")}))
        .collect::<Vec<_>>();
    Ok(ok(json!({"functionId": function_id, "slots": slots})))
}

fn secrets(session: &Session, args: &Value) -> Value {
    let hits = hbc_decomp::scan_secrets(&session.file, &[]);
    let redact = !bool_arg(args, "show_full", false);
    ok(hbc_decomp::format_secrets_report(&hits, redact))
}

fn emit_hasm(session: &Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let output = hbc_decomp::emit_hasm_function(&session.file, &session.format, function_id)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    Ok(ok(output))
}

fn patch_string(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let output_path = required_string(args, "output_path")?;
    let new_value = required_string(args, "new_value")?;
    let mut file = session.file.clone();
    let output = if let Some(id) = optional_u32(args, "id")? {
        hbc_decomp::patch_string_by_id(
            &mut file,
            &session.format,
            id,
            &new_value,
            &hbc_decomp::PatchOptions::default(),
        )
    } else {
        let old_value = required_string(args, "old_value")?;
        hbc_decomp::patch_string_replace(
            &mut file,
            &session.format,
            &old_value,
            &new_value,
            &hbc_decomp::PatchOptions::default(),
        )
    }
    .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    std::fs::write(&output_path, output)
        .map_err(|e| error("IO_ERROR", format!("Failed to write {output_path}: {e}")))?;
    Ok(ok(json!({"path": output_path})))
}

fn patch_function(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let output_path = required_string(args, "output_path")?;
    let hasm = required_string(args, "hasm")?;
    let mut file = session.file.clone();
    let instructions = hbc_decomp::parse_hasm_with_context(&hasm, &session.format, &file)
        .map_err(|e| error("INVALID_ARGUMENT", e.to_string()))?;
    let output = hbc_decomp::patch_function_body(
        &mut file,
        &session.format,
        function_id,
        &instructions,
        &hbc_decomp::PatchOptions::default(),
    )
    .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    std::fs::write(&output_path, output)
        .map_err(|e| error("IO_ERROR", format!("Failed to write {output_path}: {e}")))?;
    Ok(ok(json!({"path": output_path})))
}

fn inject_stub(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let output_path = required_string(args, "output_path")?;
    let kind = match string_arg(args, "kind", "nop").as_str() {
        "nop" => hbc_decomp::InjectStubKind::NopPad,
        "log" => hbc_decomp::InjectStubKind::LogEntry,
        value => return Err(error("INVALID_ARGUMENT", format!("Unknown stub kind: {value}"))),
    };
    let mut file = session.file.clone();
    let output = hbc_decomp::inject_stub(
        &mut file,
        &session.format,
        function_id,
        kind,
        &hbc_decomp::PatchOptions::default(),
    )
    .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    std::fs::write(&output_path, output)
        .map_err(|e| error("IO_ERROR", format!("Failed to write {output_path}: {e}")))?;
    Ok(ok(json!({"path": output_path})))
}

fn create_file(args: &Value) -> Result<Value, Value> {
    let output_path = required_string(args, "output_path")?;
    let version = args
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(96);
    let strings = args
        .get("strings")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["global".to_string()]);
    let bytes = hbc_decomp::create_minimal(&hbc_decomp::CreateOptions {
        version,
        global_body: Vec::new(),
        strings,
    })
    .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    std::fs::write(&output_path, bytes)
        .map_err(|e| error("IO_ERROR", format!("Failed to write {output_path}: {e}")))?;
    Ok(ok(json!({"path": output_path, "version": version})))
}

fn frida_hooks(session: &Session, args: &Value) -> Result<Value, Value> {
    let output_dir = required_string(args, "output_dir")?;
    let module_id = required_u32(args, "module_id")?;
    let exports = args
        .get("exports")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    let bundle = hbc_decomp::generate_frida_for_file(
        &session.file,
        &session.format,
        hbc_decomp::FridaHookOptions {
            module_id,
            exports,
            ..Default::default()
        },
    )
    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let directory = Path::new(&output_dir);
    std::fs::create_dir_all(directory)
        .map_err(|e| error("IO_ERROR", format!("Failed to create {output_dir}: {e}")))?;
    for (name, body) in [
        ("before.js", bundle.before_js),
        ("after.js", bundle.after_js),
        ("agent.js", bundle.agent_js),
        ("run.sh", bundle.run_sh),
    ] {
        std::fs::write(directory.join(name), body)
            .map_err(|e| error("IO_ERROR", format!("Failed to write {name}: {e}")))?;
    }
    Ok(ok(json!({"outputDir": output_dir, "moduleId": module_id})))
}

/// Dispatches the Android-facing command protocol. Arguments are deliberately
/// JSON values so the Kotlin API can expose the full decompiler without adding
/// a new JNI method for every decompiler feature.
pub fn dispatch(session: Option<&mut Session>, command: &str, arguments: Value) -> Value {
    match command {
        "info" | "file-info" => session
            .map(|session| file_info(session))
            .unwrap_or_else(|| error("INVALID_HANDLE", "A session is required")),
        "list-functions" => session
            .map(|session| list_functions(session, &arguments))
            .unwrap_or_else(|| error("INVALID_HANDLE", "A session is required")),
        "modules" | "list-modules" => session
            .map(|session| modules(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "disasm" | "disassembleFunction" => session
            .map(|session| disassemble(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "decompile" | "decompileFunction" => session
            .map(|session| decompile_function(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "decompile-full" | "decompileFunctionFull" => session
            .map(|session| full_decompile_function(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "decompile-all" => session
            .map(decompile_all)
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "xref" | "search-strings" | "search-functions" => session
            .map(|session| xrefs(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "graph" | "graphviz" | "getControlFlowGraph" => session
            .map(|session| graphviz(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "callgraph" => session
            .map(|session| callgraph(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "closures" => session
            .map(|session| closures(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "dump" => session
            .map(|session| dump(session, &arguments))
            .unwrap_or_else(|| error("INVALID_HANDLE", "A session is required")),
        "dump-table" | "dumpTable" => session
            .map(|session| dump_table(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "deps" | "module-deps" => session
            .map(|session| dependencies(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "secrets" | "scanSecrets" => session
            .map(|session| secrets(session, &arguments))
            .unwrap_or_else(|| error("INVALID_HANDLE", "A session is required")),
        "emit-hasm" => session
            .map(|session| emit_hasm(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "patch-string" => session
            .map(|session| patch_string(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "patch-function" | "patchFunction" => session
            .map(|session| patch_function(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "inject-stub" => session
            .map(|session| inject_stub(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "frida-hooks" | "generateFridaHooks" => session
            .map(|session| frida_hooks(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "create" => create_file(&arguments).unwrap_or_else(|value| value),
        "list-versions" => ok(json!({"min": 40, "max": 99, "versions": hbc_decomp::opcode::available_versions()})),
        _ => error("UNKNOWN_COMMAND", format!("Unknown command: {command}")),
    }
}
