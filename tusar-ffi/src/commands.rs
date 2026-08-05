use serde_json::{json, Value};

use crate::session::Session;

pub fn dispatch(
    session: Option<&mut Session>,
    command: &str,
    arguments: Value,
) -> Value {
    match command {
        "info" => info(session, arguments),

        "modules" | "list-functions" => {
            list_functions(session, arguments)
        }

        "disasm" | "disassembleFunction" => {
            disassemble_function(session, arguments)
        }

        "decompile" | "decompileFunction" => {
            decompile_function(session, arguments)
        }

        "search-strings" | "searchStrings" => {
            search_strings(session, arguments)
        }

        "search-functions" | "searchFunctions" => {
            search_functions(session, arguments)
        }

        "graph" | "getControlFlowGraph" => {
            control_flow_graph(session, arguments)
        }

        "xref" => xref(session, arguments),

        "callgraph" => callgraph(session, arguments),

        "closures" => closures(session, arguments),

        "dump" => dump(session, arguments),

        "deps" => dependencies(session, arguments),

        "secrets" | "scanSecrets" => {
            scan_secrets(session, arguments)
        }

        "frida-hooks" | "generateFridaHooks" => {
            frida_hooks(session, arguments)
        }

        "emit-hasm" => emit_hasm(session, arguments),

        "asm" => assemble(session, arguments),

        "asm-check" => assembly_check(session, arguments),

        "patch-string" => patch_string(session, arguments),

        "patch-function" | "patchFunction" => {
            patch_function(session, arguments)
        }

        "inject-stub" => inject_stub(session, arguments),

        "create" => create_file(arguments),

        "extract" => extract(session, arguments),

        "bin-diff" => binary_diff(arguments),

        "debug" => debug_dump(session, arguments),

        _ => json!({
            "ok": false,
            "error": {
                "code": "UNKNOWN_COMMAND",
                "message": format!("Unknown command: {}", command)
            }
        }),
    }
}
