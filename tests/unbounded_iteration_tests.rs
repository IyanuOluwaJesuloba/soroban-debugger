use soroban_debugger::analyzer::security::{ SecurityAnalyzer, ConfidenceLevel };
use soroban_debugger::server::protocol::{ DynamicTraceEvent, DynamicTraceEventKind };
use std::default::Default;

fn uleb128(mut value: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

fn encode_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&uleb128(value.len()));
    bytes.extend_from_slice(value.as_bytes());
}

fn append_section(module: &mut Vec<u8>, id: u8, section: &[u8]) {
    module.push(id);
    module.extend_from_slice(&uleb128(section.len()));
    module.extend_from_slice(section);
}

fn make_wasm_with_storage_in_loop(storage_import_name: &str) -> Vec<u8> {
    let mut module = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    // Type section: one () -> () function type.
    let mut ty = Vec::new();
    ty.extend_from_slice(&uleb128(1));
    ty.push(0x60);
    ty.push(0x00);
    ty.push(0x00);
    append_section(&mut module, 1, &ty);

    // Import section: import storage function.
    let mut import = Vec::new();
    import.extend_from_slice(&uleb128(1));
    encode_string(&mut import, "env");
    encode_string(&mut import, storage_import_name);
    import.push(0x00); // import kind: function
    import.extend_from_slice(&uleb128(0)); // type index 0
    append_section(&mut module, 2, &import);

    // Function section: one local function using type index 0.
    let mut functions = Vec::new();
    functions.extend_from_slice(&uleb128(1));
    functions.extend_from_slice(&uleb128(0));
    append_section(&mut module, 3, &functions);

    // Export section: export local function at index 1 (import is index 0).
    let mut exports = Vec::new();
    exports.extend_from_slice(&uleb128(1));
    encode_string(&mut exports, "entrypoint");
    exports.push(0x00); // export kind: function
    exports.extend_from_slice(&uleb128(1));
    append_section(&mut module, 7, &exports);

    // Code section: body = loop ... call storage ... end
    let mut code = Vec::new();
    code.extend_from_slice(&uleb128(1)); // one body
    let body = vec![
        0x00, // no locals
        0x03, // loop
        0x40, // empty block type
        0x10,
        0x00, // call imported function index 0 (storage)
        0x0b, // end loop
        0x0b // end function
    ];
    code.extend_from_slice(&uleb128(body.len()));
    code.extend_from_slice(&body);
    append_section(&mut module, 10, &code);

    module
}

fn make_wasm_with_nested_storage_loops() -> Vec<u8> {
    let mut module = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    // Type section
    let mut ty = Vec::new();
    ty.extend_from_slice(&uleb128(1));
    ty.push(0x60);
    ty.push(0x00);
    ty.push(0x00);
    append_section(&mut module, 1, &ty);

    // Import section: import storage function.
    let mut import = Vec::new();
    import.extend_from_slice(&uleb128(1));
    encode_string(&mut import, "env");
    encode_string(&mut import, "storage_get");
    import.push(0x00);
    import.extend_from_slice(&uleb128(0));
    append_section(&mut module, 2, &import);

    // Function section
    let mut functions = Vec::new();
    functions.extend_from_slice(&uleb128(1));
    functions.extend_from_slice(&uleb128(0));
    append_section(&mut module, 3, &functions);

    // Export section
    let mut exports = Vec::new();
    exports.extend_from_slice(&uleb128(1));
    encode_string(&mut exports, "nested_loop_test");
    exports.push(0x00);
    exports.extend_from_slice(&uleb128(1));
    append_section(&mut module, 7, &exports);

    // Code section: nested loops with storage calls
    let mut code = Vec::new();
    code.extend_from_slice(&uleb128(1));
    let body = vec![
        0x00, // no locals
        0x03, // outer loop
        0x40, // empty block type
        0x03, // inner loop
        0x40, // empty block type
        0x10,
        0x00, // call storage in inner loop
        0x10,
        0x00, // another call storage in inner loop
        0x0b, // end inner loop
        0x10,
        0x00, // call storage in outer loop
        0x0b, // end outer loop
        0x0b // end function
    ];
    code.extend_from_slice(&uleb128(body.len()));
    code.extend_from_slice(&body);
    append_section(&mut module, 10, &code);

    module
}

fn make_wasm_with_storage_outside_loop() -> Vec<u8> {
    let mut module = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

    // Type section
    let mut ty = Vec::new();
    ty.extend_from_slice(&uleb128(1));
    ty.push(0x60);
    ty.push(0x00);
    ty.push(0x00);
    append_section(&mut module, 1, &ty);

    // Import section: import storage function.
    let mut import = Vec::new();
    import.extend_from_slice(&uleb128(1));
    encode_string(&mut import, "env");
    encode_string(&mut import, "storage_get");
    import.push(0x00);
    import.extend_from_slice(&uleb128(0));
    append_section(&mut module, 2, &import);

    // Function section
    let mut functions = Vec::new();
    functions.extend_from_slice(&uleb128(1));
    functions.extend_from_slice(&uleb128(0));
    append_section(&mut module, 3, &functions);

    // Export section
    let mut exports = Vec::new();
    exports.extend_from_slice(&uleb128(1));
    encode_string(&mut exports, "safe_function");
    exports.push(0x00);
    exports.extend_from_slice(&uleb128(1));
    append_section(&mut module, 7, &exports);

    // Code section: storage call outside loop, loop without storage
    let mut code = Vec::new();
    code.extend_from_slice(&uleb128(1));
    let body = vec![
        0x00, // no locals
        0x10,
        0x00, // call storage outside loop
        0x03, // loop
        0x40, // empty block type
        0x41,
        0x01, // const 1
        0x41,
        0x01, // const 1
        0x6a, // i32.add
        0x0b, // end loop
        0x0b // end function
    ];
    code.extend_from_slice(&uleb128(body.len()));
    code.extend_from_slice(&body);
    append_section(&mut module, 10, &code);

    module
}

fn has_unbounded_iteration_finding(wasm: &[u8]) -> bool {
    let analyzer = SecurityAnalyzer::new();
    let report = analyzer.analyze(wasm, None, None).expect("analysis failed");
    report.findings.iter().any(|f| f.rule_id == "unbounded-iteration")
}

fn get_unbounded_iteration_finding(
    wasm: &[u8]
) -> Option<soroban_debugger::analyzer::security::SecurityFinding> {
    let analyzer = SecurityAnalyzer::new();
    let report = analyzer.analyze(wasm, None, None).expect("analysis failed");
    report.findings.into_iter().find(|f| f.rule_id == "unbounded-iteration")
}

#[test]
fn detects_storage_call_in_simple_loop() {
    let wasm = make_wasm_with_storage_in_loop("storage_get");
    assert!(has_unbounded_iteration_finding(&wasm));

    let finding = get_unbounded_iteration_finding(&wasm).unwrap();
    assert!(matches!(finding.severity, soroban_debugger::analyzer::security::Severity::High));

    // Check confidence level
    let confidence = finding.confidence.as_ref().unwrap();
    assert!(matches!(confidence.level, ConfidenceLevel::Low)); // Single call, shallow nesting

    // Check context
    let context = finding.context.as_ref().unwrap();
    assert_eq!(context.loop_nesting_depth, Some(1));
    assert!(context.storage_call_pattern.is_some());
}

#[test]
fn detects_nested_storage_loops_with_high_confidence() {
    let wasm = make_wasm_with_nested_storage_loops();
    assert!(has_unbounded_iteration_finding(&wasm));

    let finding = get_unbounded_iteration_finding(&wasm).unwrap();

    // Should have high confidence due to multiple calls and nesting
    let confidence = finding.confidence.as_ref().unwrap();
    assert!(matches!(confidence.level, ConfidenceLevel::High));

    let context = finding.context.as_ref().unwrap();
    assert_eq!(context.loop_nesting_depth, Some(2));

    let pattern = context.storage_call_pattern.as_ref().unwrap();
    assert_eq!(pattern.calls_in_loops, 3);
    assert_eq!(pattern.calls_outside_loops, 0);
}

#[test]
fn does_not_trigger_on_storage_outside_loops() {
    let wasm = make_wasm_with_storage_outside_loop();
    assert!(!has_unbounded_iteration_finding(&wasm));
}

#[test]
fn detects_various_storage_import_names() {
    let storage_imports = [
        "storage_get",
        "storage_has",
        "storage_next",
        "storage_iter",
        "contract_storage_get",
        "soroban_storage_has",
    ];

    for import_name in storage_imports {
        let wasm = make_wasm_with_storage_in_loop(import_name);
        assert!(
            has_unbounded_iteration_finding(&wasm),
            "Should detect storage import: {}",
            import_name
        );
    }
}

#[test]
fn ignores_non_storage_imports_in_loops() {
    let non_storage_imports = ["invoke_contract", "transfer", "bytes_new", "val_to_object"];

    for import_name in non_storage_imports {
        let wasm = make_wasm_with_storage_in_loop(import_name);
        assert!(
            !has_unbounded_iteration_finding(&wasm),
            "Should not detect non-storage import: {}",
            import_name
        );
    }
}

#[test]
fn provides_rich_context_in_findings() {
    let wasm = make_wasm_with_nested_storage_loops();
    let finding = get_unbounded_iteration_finding(&wasm).unwrap();

    // Check that context is provided
    assert!(finding.context.is_some());
    let context = finding.context.as_ref().unwrap();

    // Check control flow info
    assert!(context.control_flow_info.is_some());
    let cf_info = context.control_flow_info.as_ref().unwrap();
    assert!(!cf_info.loop_types.is_empty());

    // Check storage call pattern
    assert!(context.storage_call_pattern.is_some());
    let pattern = context.storage_call_pattern.as_ref().unwrap();
    assert_eq!(pattern.calls_in_loops, 3);
    assert!(
        pattern.loop_types_with_calls.contains(&"top_level_loop".to_string()) ||
            pattern.loop_types_with_calls.contains(&"nested_loop".to_string())
    );

    // Check confidence rationale
    let confidence = finding.confidence.as_ref().unwrap();
    assert!(!confidence.rationale.is_empty());
    assert!(confidence.rationale.contains("Storage calls in loops"));
}

#[test]
fn dynamic_analysis_detects_high_storage_pressure() {
    let mut trace = Vec::new();

    // Create a trace with many storage reads (simulating unbounded iteration)
    for i in 0..100 {
        trace.push(DynamicTraceEvent {
            sequence: i as usize,
            kind: DynamicTraceEventKind::StorageRead,
            message: String::new(),
            function: None,
            storage_key: Some(format!("key_{}", i % 10)), // Only 10 unique keys
            storage_value: None,
            call_depth: 0,
        });
    }

    let analyzer = SecurityAnalyzer::new();
    let report = analyzer.analyze(&[], None, Some(&trace)).expect("analysis failed");

    let unbounded_findings: Vec<_> = report.findings
        .iter()
        .filter(|f| f.rule_id == "unbounded-iteration")
        .collect();

    assert!(!unbounded_findings.is_empty(), "Should detect high storage pressure in dynamic trace");

    let finding = &unbounded_findings[0];
    assert!(finding.description.contains("detected with"), "Expected finding description to indicate detection: {}", finding.description);
}

#[test]
fn dynamic_analysis_ignores_reasonable_storage_access() {
    let mut trace = Vec::new();

    // Create a trace with reasonable storage access
    for i in 0..10 {
        trace.push(DynamicTraceEvent {
            sequence: i as usize,
            kind: DynamicTraceEventKind::StorageRead,
            message: String::new(),
            function: None,
            storage_key: Some(format!("key_{}", i)), // 10 unique keys
            storage_value: None,
            call_depth: 0,
        });
    }

    let analyzer = SecurityAnalyzer::new();
    let report = analyzer.analyze(&[], None, Some(&trace)).expect("analysis failed");

    let unbounded_findings: Vec<_> = report.findings
        .iter()
        .filter(|f| f.rule_id == "unbounded-iteration")
        .collect();

    assert!(unbounded_findings.is_empty(), "Should not flag reasonable storage access");
}
