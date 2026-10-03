//! Registered module loading through a controlled host; no provider calls.

mod common;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use common::{FakeHost, error, texts, tool};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_codemode::{
    CancellationToken,
    Failure,
    Host,
    Namespace,
    Outcome,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    modules::{Definition, Library, Record},
    options,
    run,
    store::Snapshot,
};

struct ModuleHost {
    tools: FakeHost,
    library: Library,
    lookups: AtomicUsize,
    initializations: Mutex<Vec<(String, String)>>,
}

impl ModuleHost {
    fn new(definitions: &[Definition]) -> Arc<Self> {
        let mut library = Library::default();
        for definition in definitions {
            library
                .apply(&Record::Define {
                    definition: definition.clone(),
                })
                .unwrap();
        }
        Arc::new(Self {
            tools: FakeHost::default(),
            library,
            lookups: AtomicUsize::new(0),
            initializations: Mutex::new(Vec::new()),
        })
    }

    async fn script(self: &Arc<Self>, source: &str) -> Outcome {
        self.script_with(source, CancellationToken::new()).await
    }

    async fn script_with(
        self: &Arc<Self>,
        source: &str,
        cancel: CancellationToken,
    ) -> Outcome {
        run(
            self.clone(),
            Request {
                call_id: "module_test".into(),
                source: options::parse(source).unwrap(),
                store: Snapshot::new(),
                cancel,
            },
        )
        .await
    }
}

#[async_trait]
impl Host for ModuleHost {
    fn tools(&self) -> Vec<ToolEntry> {
        let mut tools = self.tools.tools();
        tools.push(tool("init", "Records a module source initialization."));
        tools
    }
    fn namespaces(&self) -> Vec<Namespace> {
        self.tools.namespaces()
    }
    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        if call.name == "init" {
            let name = call.args["name"]
                .as_str()
                .ok_or("init requires a name")?
                .to_owned();
            let label = call.args["label"]
                .as_str()
                .ok_or("init requires a label")?
                .to_owned();
            self.initializations.lock().unwrap().push((name, label));
            return Ok(ToolReply {
                value: call.args,
                error: None,
                usage: None,
                usage_complete: None,
            });
        }
        self.tools.call_tool(call).await
    }
    async fn module(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Definition>, String> {
        self.lookups.fetch_add(1, Ordering::SeqCst);
        let version = version
            .or_else(|| self.library.selected().get(name).map(String::as_str));
        Ok(version
            .and_then(|version| self.library.versions().get(version))
            .filter(|definition| definition.name() == name)
            .cloned())
    }
}

fn definition(
    name: &str,
    source: &str,
    dependencies: &[(&str, &str)],
) -> Definition {
    Definition::new(
        name.into(),
        source.into(),
        json!({}),
        dependencies
            .iter()
            .map(|(name, version)| ((*name).into(), (*version).into()))
            .collect::<BTreeMap<_, _>>(),
    )
    .unwrap()
}

#[tokio::test]
async fn loads_function_and_table_from_exact_versions() {
    let function =
        definition("increment", "return function(n) return n + 1 end", &[]);
    let table = definition("constants", "return { answer = 42 }", &[]);
    let host = ModuleHost::new(&[function.clone(), table.clone()]);
    let output = host.script(&format!(
        "return require('increment', '{}')(2), require('constants', '{}').answer",
        function.version(), table.version()
    )).await;
    assert_eq!(output.failure, None);
    assert_eq!(texts(&output), ["3", "42"]);
}

#[tokio::test]
async fn dependencies_use_pins_even_after_selection_changes() {
    let old = definition("base", "return { n = 1 }", &[]);
    let new = definition("base", "return { n = 9 }", &[]);
    let parent = definition(
        "parent",
        "return { n = require('base').n }",
        &[("base", old.version())],
    );
    let host = ModuleHost::new(&[old.clone(), new, parent]);
    let output = host
        .script("return require('parent').n, require('base').n")
        .await;
    assert_eq!(texts(&output), ["1", "9"]);
    assert_eq!(output.failure, None);
}

#[tokio::test]
async fn rejects_missing_names_paths_cycles_and_mismatched_pins() {
    let missing_pin = "0".repeat(64);
    let missing =
        definition("missing_dep", "return {}", &[("other", &missing_pin)]);
    let base = definition("base", "return {}", &[]);
    let mismatch = definition(
        "mismatch",
        &format!("return require('base', '{}')", missing_pin),
        &[("base", base.version())],
    );
    let host = ModuleHost::new(&[missing, base, mismatch]);
    for (code, expected) in [
        ("require('unknown')", "not registered"),
        ("require('../secret')", "module name"),
        ("require('missing_dep')", "not registered"),
        ("require('mismatch')", "version mismatch"),
    ] {
        let output = host.script(&format!("local ok, message = pcall(function() {code} end); return ok, message")).await;
        assert_eq!(output.failure, None, "{output:?}");
        assert_eq!(texts(&output)[0], "false");
        assert!(texts(&output)[1].contains(expected), "{output:?}");
    }
    // The digest includes dependency pins, so a real cyclic pair cannot be
    // constructed from Definition::new without solving a hash fixed point.
    // The graph walker still checks the active dependency chain explicitly.
}

#[tokio::test]
async fn rejects_a_forged_self_cycle_before_running_source() {
    struct ForgedHost(Definition);
    #[async_trait]
    impl Host for ForgedHost {
        fn tools(&self) -> Vec<ToolEntry> {
            Vec::new()
        }
        async fn call_tool(&self, _: ToolCall) -> Result<ToolReply, String> {
            unreachable!("the forged source must not run")
        }
        async fn module(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> Result<Option<Definition>, String> {
            Ok(Some(self.0.clone()))
        }
    }
    let mut forged =
        serde_json::to_value(definition("cycle", "error('ran')", &[])).unwrap();
    let pin = "a".repeat(64);
    forged["version"] = json!(pin);
    forged["dependencies"] = json!({ "cycle": pin });
    let host: Arc<dyn Host> =
        Arc::new(ForgedHost(serde_json::from_value(forged).unwrap()));
    let output = run(
        host,
        Request {
            call_id: "forged_cycle".into(),
            source: options::parse("require('cycle')").unwrap(),
            store: Snapshot::new(),
            cancel: CancellationToken::new(),
        },
    )
    .await;
    assert!(
        error(&output).contains("version does not match its content"),
        "{output:?}"
    );
}

#[tokio::test]
async fn syntax_errors_and_invalid_exports_are_string_catchable() {
    let syntax = definition("syntax", "return function(", &[]);
    let nil = definition("nil_export", "return nil", &[]);
    let many = definition("many", "return {}, {}", &[]);
    let host = ModuleHost::new(&[syntax, nil, many]);
    for (name, expected) in [
        ("syntax", "module:syntax@"),
        ("nil_export", "function or table"),
        ("many", "exactly one"),
    ] {
        let output = host
            .script(&format!(
                "local ok, e = pcall(require, '{name}'); return ok, e"
            ))
            .await;
        assert_eq!(output.failure, None, "{output:?}");
        assert_eq!(texts(&output)[0], "false");
        assert!(texts(&output)[1].contains(expected), "{output:?}");
    }
}

#[tokio::test]
async fn module_functions_call_tools_and_heaps_are_fresh_per_vm() {
    let module = definition(
        "counter",
        "local n = 0; return function() n += 1; return tools.echo({ n = n }).n end",
        &[],
    );
    let host = ModuleHost::new(&[module]);
    let code = "local f = require('counter'); return f(), f()";
    let first = host.script(code).await;
    let second = host.script(code).await;
    assert_eq!(texts(&first), ["1", "2"]);
    assert_eq!(texts(&second), ["1", "2"]);
    assert_eq!(host.tools.names(), ["echo", "echo", "echo", "echo"]);
}

#[tokio::test]
async fn modules_cannot_import_files_or_undeclared_modules() {
    let other = definition("other", "return {}", &[]);
    let module = definition(
        "guard",
        "return { io = io, package = package, require_other = function() return require('other') end }",
        &[],
    );
    let host = ModuleHost::new(&[other, module]);
    let output = host.script("local g = require('guard'); local ok, e = pcall(g.require_other); return g.io == nil, g.package == nil, ok, e, loadfile == nil").await;
    assert_eq!(output.failure, None);
    assert_eq!(texts(&output)[..3], ["true", "true", "false"]);
    assert!(texts(&output)[3].contains("not declared"));
    assert_eq!(texts(&output)[4], "true");
}

#[tokio::test]
async fn deleting_environment_fields_cannot_restore_caller_introspection() {
    let module = definition(
        "guard",
        "getfenv = nil; rawset(_G, 'setfenv', nil); return { get = getfenv, set = setfenv }",
        &[],
    );
    let host = ModuleHost::new(&[module]);
    let output = host
        .script(
            "local g = require('guard'); return g.get == false, g.set == false",
        )
        .await;
    assert_eq!(output.failure, None);
    assert_eq!(texts(&output), ["true", "true"]);
}

#[tokio::test]
async fn timeout_and_cancellation_stop_module_sources_and_calls() {
    let loop_module = definition("loop_module", "while true do end", &[]);
    let call_module = definition(
        "call_module",
        "return function() return tools.sleep({ ms = 10000 }) end",
        &[],
    );
    let host = ModuleHost::new(&[loop_module, call_module]);
    let timeout = host
        .script("-- @options: {\"timeout_ms\": 100}\nrequire('loop_module')")
        .await;
    assert_eq!(timeout.failure, Some(Failure::TimedOut { timeout_ms: 100 }));
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let cancelled = host.script_with("require('call_module')()", cancel).await;
    assert_eq!(cancelled.failure, Some(Failure::Cancelled));
    assert_eq!(host.tools.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn repeated_and_concurrent_require_share_one_module_value() {
    let module = definition(
        "shared",
        "local n = 0; return function() n += 1; return n end",
        &[],
    );
    let host = ModuleHost::new(&[module]);
    let output = host.script("local a, b = parallel(function() return require('shared') end, function() return require('shared') end); return a == b, a(), b(), require('shared')()").await;
    assert_eq!(output.failure, None, "{output:?}");
    assert_eq!(texts(&output), ["true", "1", "2", "3"]);
}

#[tokio::test]
async fn arithmetic_module_returns_the_fixed_example() {
    let module = definition(
        "arithmetic",
        "return function(a, b) return a * 3 + b * 2 end",
        &[],
    );
    let host = ModuleHost::new(&[module]);
    let output = host.script("return require('arithmetic')(3, 2)").await;
    assert_eq!(output.failure, None);
    assert_eq!(texts(&output), ["13"]);
}

fn graph_edges(optional_edges: u8) -> Vec<Vec<usize>> {
    // Indices 0..4 are leaves. Nodes 5 and 6 share leaf 0; root 7
    // requires both, so every generated graph contains a diamond.
    let mut edges = vec![Vec::new(); 8];
    edges[5] = vec![0, 1];
    edges[6] = vec![0, 2];
    edges[7] = vec![5, 6];
    for (bit, parent, child) in [
        (0, 5, 2),
        (1, 5, 3),
        (2, 5, 4),
        (3, 6, 1),
        (4, 6, 3),
        (5, 6, 4),
        (6, 7, 3),
        (7, 7, 4),
    ] {
        if optional_edges & (1 << bit) != 0 {
            edges[parent].push(child);
        }
    }
    edges
}

fn graph_module(
    name: &str,
    label: &str,
    base: i32,
    children: &[usize],
    definitions: &[Definition],
) -> Definition {
    let mut source = format!(
        "local init = tools.init({{ name = '{name}', label = '{label}' }})\nlocal value = {base}"
    );
    let mut dependencies = Vec::new();
    for &child in children {
        let dependency = &definitions[child];
        let dependency_name = dependency.name();
        source.push_str(&format!(
            "\nvalue += require('{dependency_name}').value"
        ));
        dependencies.push((dependency_name, dependency.version()));
    }
    source.push_str("\nreturn { value = value, label = init.label }");
    definition(name, &source, &dependencies)
}

fn expected_graph_values(edges: &[Vec<usize>], bases: &[i32; 8]) -> [i32; 8] {
    let mut values = [0; 8];
    for node in 0..8 {
        values[node] = bases[node]
            + edges[node].iter().map(|&child| values[child]).sum::<i32>();
    }
    values
}

fn expected_initializations(
    edges: &[Vec<usize>],
) -> BTreeMap<(String, String), usize> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![7];
    while let Some(node) = pending.pop() {
        if reachable.insert(node) {
            pending.extend(edges[node].iter().copied());
        }
    }
    let mut counts = BTreeMap::new();
    for node in reachable {
        if node != 7 {
            let name = format!("node{node}");
            counts.insert((name.clone(), name), 1);
        }
    }
    counts.insert(("root".into(), "old".into()), 1);
    counts.insert(("root".into(), "new".into()), 1);
    counts
}

fn count_initializations(
    events: &[(String, String)],
) -> BTreeMap<(String, String), usize> {
    let mut counts = BTreeMap::new();
    for event in events {
        *counts.entry(event.clone()).or_insert(0) += 1;
    }
    counts
}

// Property inventory: exact imports and a later bare import return the
// independently summed graph values, while each reachable exact version
// executes once per VM. Five fixed leaves, a forced diamond, two root versions,
// and eight optional earlier-node edges construct valid DAGs without filtering;
// the edge mask shrinks toward the minimal diamond and bases toward one.
#[hegel::test(test_cases = 24)]
fn generated_exact_definition_graphs_preserve_values_pins_and_vm_cache(
    tc: TestCase,
) {
    let optional_edges: u8 = tc.draw(gs::integers());
    let left_base: i32 = tc.draw(gs::integers().min_value(1).max_value(8));
    let right_base: i32 = tc.draw(gs::integers().min_value(1).max_value(8));
    let old_root_base: i32 = tc.draw(gs::integers().min_value(1).max_value(8));
    let edges = graph_edges(optional_edges);
    let bases = [0, 1, 2, 3, 4, left_base, right_base, old_root_base];
    let values = expected_graph_values(&edges, &bases);
    let new_root_value = values[7] + 1;

    let mut definitions = Vec::new();
    for node in 0..7 {
        let name = format!("node{node}");
        definitions.push(graph_module(
            &name,
            &name,
            bases[node],
            &edges[node],
            &definitions,
        ));
    }
    let old_root =
        graph_module("root", "old", old_root_base, &edges[7], &definitions);
    let new_root =
        graph_module("root", "new", old_root_base + 1, &edges[7], &definitions);
    let script = format!(
        "local old = require('root', '{}')\n\
         local newer = require('root', '{}')\n\
         local current = require('root')\n\
         return old.value, newer.value, current.value, old.label, newer.label, current.label, \
         old == require('root', '{}'), current == require('root'), newer == require('root', '{}'), \
         current == newer, old ~= newer",
        old_root.version(),
        new_root.version(),
        old_root.version(),
        new_root.version(),
    );
    definitions.push(old_root);
    definitions.push(new_root);
    let host = ModuleHost::new(&definitions);
    let expected_counts = expected_initializations(&edges);
    let expected_text = [
        values[7].to_string(),
        new_root_value.to_string(),
        new_root_value.to_string(),
        "old".into(),
        "new".into(),
        "new".into(),
        "true".into(),
        "true".into(),
        "true".into(),
        "true".into(),
        "true".into(),
    ];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for _ in 0..2 {
        let previous = host.initializations.lock().unwrap().len();
        let output = runtime.block_on(host.script(&script));
        assert_eq!(output.failure, None, "{output:?}");
        assert_eq!(texts(&output), expected_text);
        let events = host.initializations.lock().unwrap();
        assert_eq!(count_initializations(&events[previous..]), expected_counts);
    }
}
