use hbc_decomp::{BytecodeFile, DecompileOptionsV2, DisasmOptions};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::Session;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(1);

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

// Return a normalized output path and reject accidental writes to the input.
// Existing outputs require an explicit `overwrite: true` argument.
fn safe_output_path(session: &Session, args: &Value) -> Result<PathBuf, Value> {
    let raw = required_string(args, "output_path")?;
    let path = PathBuf::from(&raw);
    if raw.trim().is_empty() || path.file_name().is_none() {
        return Err(error("INVALID_ARGUMENT", "output_path must name a file"));
    }
    let input = fs::canonicalize(&session.input_path)
        .map_err(|e| error("IO_ERROR", format!("Cannot resolve input path: {e}")))?;
    let output_existing = fs::canonicalize(&path).ok();
    if output_existing.as_ref() == Some(&input) {
        return Err(error(
            "UNSAFE_OUTPUT",
            "output_path must not overwrite the input bundle",
        ));
    }
    if path.exists() && !bool_arg(args, "overwrite", false) {
        return Err(error(
            "OUTPUT_EXISTS",
            "Refusing to replace an existing output; pass overwrite=true explicitly",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(error(
            "IO_ERROR",
            format!("Output directory does not exist: {}", parent.display()),
        ));
    }
    Ok(path)
}

fn standalone_output_path(args: &Value) -> Result<PathBuf, Value> {
    let raw = required_string(args, "output_path")?;
    let path = PathBuf::from(&raw);
    if raw.trim().is_empty() || path.file_name().is_none() {
        return Err(error("INVALID_ARGUMENT", "output_path must name a file"));
    }
    if path.exists() && !bool_arg(args, "overwrite", false) {
        return Err(error(
            "OUTPUT_EXISTS",
            "Refusing to replace an existing output; pass overwrite=true explicitly",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(error(
            "IO_ERROR",
            format!("Output directory does not exist: {}", parent.display()),
        ));
    }
    Ok(path)
}

// Write bytes through a same-directory temporary file, flush/sync them, then
// rename into place. This prevents a killed Android process from leaving a
// truncated output bundle. HBC outputs are parsed and footer-checked first.
fn atomic_write(path: &Path, bytes: &[u8], validate_hbc: bool, overwrite: bool) -> Result<(), Value> {
    if validate_hbc {
        if !hbc_decomp::verify_footer(bytes) {
            return Err(error("VALIDATION_ERROR", "Generated HBC footer verification failed"));
        }
        BytecodeFile::parse_auto(bytes)
            .map_err(|e| error("VALIDATION_ERROR", format!("Generated HBC failed to parse: {e}")))?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    let temp = parent.join(format!(".{name}.tmp-{stamp:x}-{counter:x}"));

    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| error("IO_ERROR", format!("Cannot create temporary output: {e}")))?;
        file.write_all(bytes)
            .map_err(|e| error("IO_ERROR", format!("Cannot write temporary output: {e}")))?;
        file.sync_all()
            .map_err(|e| error("IO_ERROR", format!("Cannot sync temporary output: {e}")))?;
        drop(file);

        if path.exists() && !overwrite {
            return Err(error("OUTPUT_EXISTS", "Output appeared during the write"));
        }
        fs::rename(&temp, path)
            .map_err(|e| error("IO_ERROR", format!("Cannot atomically install output: {e}")))?;
        if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
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
    let tree = hbc_decomp::analysis::metro::DependencyGraph::get_dependency_tree(
        &pipeline.registry,
        module_id,
        depth,
    );
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
    let table_kind = hbc_decomp::inspect::TableKind::parse(&kind)
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

fn debug_info(session: &Session) -> Result<Value, Value> {
    let offset = session.file.header.debug_info_offset;
    let parsed = hbc_decomp::DebugInfo::parse(&session.bytes, offset)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let scopes = parsed
        .scope_descriptors
        .iter()
        .map(|scope| {
            json!({
                "offset": scope.offset,
                "parentOffset": scope.parent_offset,
                "flags": scope.flags,
                "inner": scope.is_inner_scope(),
                "dynamic": scope.is_dynamic(),
                "names": scope.names,
            })
        })
        .collect::<Vec<_>>();
    let callees = parsed
        .textified_callees
        .iter()
        .map(|(offset, name)| json!({"offset": offset, "name": name}))
        .collect::<Vec<_>>();
    let variables = parsed
        .all_variable_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Ok(ok(json!({
        "debugInfoOffset": offset,
        "scopes": scopes,
        "callees": callees,
        "variables": variables,
        "stringTable": parsed.string_table,
    })))
}

fn extract_modules(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let output_dir = required_string(args, "output_dir")?;
    session_error(session.ensure_pipeline())?;
    let pipeline = session
        .pipeline_ctx
        .as_ref()
        .ok_or_else(|| error("DECOMPILER_ERROR", "Pipeline was not initialized"))?;
    let directory = Path::new(&output_dir);
    fs::create_dir_all(directory)
        .map_err(|e| error("IO_ERROR", format!("Failed to create {output_dir}: {e}")))?;
    let mut files = Vec::new();
    for module in pipeline.registry.modules.values() {
        let filename = module
            .name
            .as_deref()
            .map(|name| name.replace(['/', '\\'], "_"))
            .map(|name| format!("{}_{}.js", module.module_id, name))
            .unwrap_or_else(|| format!("module_{}.js", module.module_id));
        let mut content = format!(
            "// Module ID: {}\n// Function ID: {}\n",
            module.module_id, module.function_id
        );
        if let Some(name) = &module.name {
            content.push_str(&format!("// Name: {name}\n"));
        }
        content.push_str(&format!("// Dependencies: {:?}\n\n", module.dependencies));
        content.push_str(&pipeline.generate_function_code(&session.file, module.function_id));
        let path = directory.join(&filename);
        fs::write(&path, content)
            .map_err(|e| error("IO_ERROR", format!("Failed to write {}: {e}", path.display())))?;
        files.push(filename);
    }
    Ok(ok(json!({"outputDir": output_dir, "count": files.len(), "files": files})))
}

fn binary_diff(session: &Session, args: &Value) -> Result<Value, Value> {
    let other_path = required_string(args, "other_path")?;
    let other_bytes = fs::read(&other_path)
        .map_err(|e| error("IO_ERROR", format!("Failed to read {other_path}: {e}")))?;
    let other_file = BytecodeFile::parse_auto(&other_bytes)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let (other_format, _) = hbc_decomp::BytecodeFormat::for_version_or_latest(other_file.header.version)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;

    let names = |file: &BytecodeFile| {
        file.function_headers
            .iter()
            .enumerate()
            .map(|(id, _)| (function_name(file, id), id as u32))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let left = names(&session.file);
    let right = names(&other_file);
    let mut all = std::collections::BTreeSet::new();
    all.extend(left.keys().cloned());
    all.extend(right.keys().cloned());
    let options = DisasmOptions {
        show_offsets: false,
        show_labels: true,
        resolve_strings: true,
        enable_color: false,
    };
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();
    let mut identical = 0u32;
    for name in all {
        match (left.get(&name), right.get(&name)) {
            (Some(&left_id), Some(&right_id)) => {
                let left_code = hbc_decomp::disassemble_function(&session.file, &session.format, left_id, &options)
                    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
                let right_code = hbc_decomp::disassemble_function(&other_file, &other_format, right_id, &options)
                    .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
                if left_code == right_code {
                    identical += 1;
                } else {
                    modified.push(json!({"name": name, "leftFunctionId": left_id, "rightFunctionId": right_id}));
                }
            }
            (Some(&left_id), None) => removed.push(json!({"name": name, "functionId": left_id})),
            (None, Some(&right_id)) => added.push(json!({"name": name, "functionId": right_id})),
            (None, None) => {}
        }
    }
    Ok(ok(json!({
        "otherPath": other_path,
        "identical": identical,
        "modified": modified,
        "removed": removed,
        "added": added,
    })))
}

fn asm_check(session: &Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let text = hbc_decomp::emit_hasm_function(&session.file, &session.format, function_id)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let parsed = hbc_decomp::parse_hasm_with_context(&text, &session.format, &session.file)
        .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    let original = session.file.decode_function_instructions(&session.format, function_id)
        .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
    let original_bytes = hbc_decomp::encode_function_body(&session.format, &original)
        .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    let roundtrip_bytes = hbc_decomp::encode_function_body(&session.format, &parsed)
        .map_err(|e| error("WRITE_ERROR", e.to_string()))?;
    Ok(ok(json!({
        "functionId": function_id,
        "match": original_bytes == roundtrip_bytes,
        "originalBytes": original_bytes.len(),
        "roundtripBytes": roundtrip_bytes.len(),
    })))
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
    let output_path = safe_output_path(session, args)?;
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
    atomic_write(&output_path, &output, true, bool_arg(args, "overwrite", false))?;
    Ok(ok(json!({"path": output_path, "bytes": output.len()})))
}

fn patch_function(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let output_path = safe_output_path(session, args)?;
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
    atomic_write(&output_path, &output, true, bool_arg(args, "overwrite", false))?;
    Ok(ok(json!({"path": output_path, "bytes": output.len()})))
}

fn patch_functions(session: &Session, args: &Value) -> Result<Value, Value> {
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .ok_or_else(|| error("INVALID_ARGUMENT", "edits must be an array"))?;
    if edits.is_empty() {
        return Err(error("INVALID_ARGUMENT", "edits must not be empty"));
    }
    let output_path = safe_output_path(session, args)?;
    let mut bytes = session
        .file
        .raw_bytes
        .clone()
        .ok_or_else(|| error("WRITE_ERROR", "Input bundle has no raw bytes"))?;
    let mut applied = Vec::with_capacity(edits.len());
    let mut seen = std::collections::BTreeSet::new();

    for (index, edit) in edits.iter().enumerate() {
        let function_id = required_u32(edit, "function_id")?;
        if !seen.insert(function_id) {
            return Err(error(
                "INVALID_ARGUMENT",
                format!("Duplicate function_id in edit list: {function_id}"),
            ));
        }
        let hasm = required_string(edit, "hasm")?;
        let mut file = BytecodeFile::parse_auto(&bytes)
            .map_err(|e| error("VALIDATION_ERROR", format!("Cannot reparse after edit {index}: {e}")))?;
        let (format, _) = hbc_decomp::BytecodeFormat::for_version_or_latest(file.header.version)
            .map_err(|e| error("DECOMPILER_ERROR", e.to_string()))?;
        let instructions = hbc_decomp::parse_hasm_with_context(&hasm, &format, &file)
            .map_err(|e| error("INVALID_ARGUMENT", format!("Edit {index}: {e}")))?;
        bytes = hbc_decomp::patch_function_body(
            &mut file,
            &format,
            function_id,
            &instructions,
            &hbc_decomp::PatchOptions::default(),
        )
        .map_err(|e| error("WRITE_ERROR", format!("Edit {index}: {e}")))?;
        applied.push(function_id);
    }

    atomic_write(&output_path, &bytes, true, bool_arg(args, "overwrite", false))?;
    Ok(ok(json!({"path": output_path, "bytes": bytes.len(), "functions": applied})))
}

fn inject_stub(session: &mut Session, args: &Value) -> Result<Value, Value> {
    let function_id = required_u32(args, "function_id")?;
    let output_path = safe_output_path(session, args)?;
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
    atomic_write(&output_path, &output, true, bool_arg(args, "overwrite", false))?;
    Ok(ok(json!({"path": output_path, "bytes": output.len()})))
}

fn create_file(args: &Value) -> Result<Value, Value> {
    let output_path = standalone_output_path(args)?;
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
    atomic_write(&output_path, &bytes, true, bool_arg(args, "overwrite", false))?;
    Ok(ok(json!({"path": output_path, "version": version, "bytes": bytes.len()})))
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
    fs::create_dir_all(directory)
        .map_err(|e| error("IO_ERROR", format!("Failed to create {output_dir}: {e}")))?;
    for (name, body) in [
        ("before.js", bundle.before_js),
        ("after.js", bundle.after_js),
        ("agent.js", bundle.agent_js),
        ("run.sh", bundle.run_sh),
    ] {
        fs::write(directory.join(name), body)
            .map_err(|e| error("IO_ERROR", format!("Failed to write {name}: {e}")))?;
    }
    Ok(ok(json!({"outputDir": output_dir, "moduleId": module_id})))
}

fn xrefs_with_kind(session: &Session, args: &Value, kind: &str) -> Result<Value, Value> {
    let mut forced = args.clone();
    let object = forced
        .as_object_mut()
        .ok_or_else(|| error("INVALID_ARGUMENT", "xref arguments must be a JSON object"))?;
    object.insert("kind".to_string(), Value::String(kind.to_string()));
    xrefs(session, &forced)
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
        "xref" => session
            .map(|session| xrefs(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "search-strings" | "searchStrings" => session
            .map(|session| xrefs_with_kind(session, &arguments, "string"))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "search-functions" | "searchFunctions" => session
            .map(|session| xrefs_with_kind(session, &arguments, "function"))
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
        "debug" | "debug-info" => session
            .map(debug_info)
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "extract" => session
            .map(|session| extract_modules(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "bin-diff" | "binary-diff" => session
            .map(|session| binary_diff(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "asm" => session
            .map(|session| patch_function(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "asm-check" => session
            .map(|session| asm_check(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "patch-functions" | "patchFunctions" => session
            .map(|session| patch_functions(session, &arguments))
            .unwrap_or_else(|| Err(error("INVALID_HANDLE", "A session is required")))
            .unwrap_or_else(|value| value),
        "tui" => error("CLI_ONLY", "The terminal UI is not available inside an Android app; use info, modules, disasm, decompile, graphviz, and callgraph instead"),
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
