//! Developer tool: list a compiled animation graph's nodes (index, class, path), optionally filtered, with its
//! control parameters — to find the states, clips and conditions that drive an animation.
//!
//! ```bash
//! cargo run --release --example graph_inspect -- animation/graphs/worldmodel/worldmodel.vnmgraph
//! cargo run --release --example graph_inspect -- animation/graphs/worldmodel/worldmodel.vnmgraph Jump
//! cargo run --release --example graph_inspect -- animation/graphs/worldmodel/worldmodel.vnmgraph CNmStateNode
//! ```
//!
//! A filter matches a path (ignoring case) or a class name. Clip nodes also print their clip path.

use ofast_core::anim::graph::Graph;
use ofast_core::content::Content;
use ofast_core::game::Cs2Install;
use ofast_core::resource::{BlockKind, Resource};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let install = Cs2Install::locate().expect("CS2 install found");
    let content = Content::game(&install).expect("archives open");
    let bytes = content.read_resource(&args[0]).expect("graph reads");
    let root = Resource::parse(&bytes)
        .expect("resource")
        .kv3(BlockKind::DATA)
        .expect("DATA")
        .root;
    // `raw [key]`: the resource's top-level keys, or one key's value (works on clips too).
    if args.get(1).map(String::as_str) == Some("raw") {
        match args.get(2) {
            Some(key) => println!(
                "{}",
                root.get(key)
                    .map_or("(missing)".into(), ofast_core::kv3::text::value_to_string)
            ),
            None => {
                for (key, _) in root.as_object().expect("object").iter() {
                    println!("{key}");
                }
            }
        }
        return;
    }
    let graph = Graph::parse(&root).expect("graph");
    // `node <index>`: one node as stored.
    if args.get(1).map(String::as_str) == Some("node") {
        for index in &args[2..] {
            let i: usize = index.parse().expect("node index");
            println!(
                "{i} {} {}\n{}",
                graph.class(i),
                graph.paths[i],
                ofast_core::kv3::text::value_to_string(&graph.nodes[i])
            );
        }
        return;
    }
    // `states <path>`: a state machine as readable rules, nested machines included.
    if args.get(1).map(String::as_str) == Some("states") {
        let machine = graph.find(&args[2]).expect("state machine path");
        print_machine(&graph, machine, 0);
        return;
    }
    let filter = args.get(1).map(|f| f.to_lowercase());
    for (i, path) in graph.paths.iter().enumerate() {
        let class = graph.class(i);
        if filter
            .as_ref()
            .is_some_and(|f| !path.to_lowercase().contains(f) && !class.to_lowercase().contains(f))
        {
            continue;
        }
        let clip = graph.clip(i).map_or(String::new(), |(c, speed)| {
            format!("  -> {c} (speed {speed})")
        });
        let clip = match graph.referenced_graph(i) {
            Some(other) => format!("{clip}  => {other}"),
            None => clip,
        };
        println!("{i:5} {class:42} {path}{clip}");
    }
    println!("parameters: {:?}", graph.parameters);
}

/// The last part of a node's path.
fn name(graph: &Graph, node: usize) -> String {
    graph.paths[node]
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

/// A value node (float, ID, bool) in words: a control parameter's name, a constant, or the node's class.
fn value(graph: &Graph, node: Option<usize>, depth: usize) -> String {
    let Some(node) = node else {
        return "?".into();
    };
    if depth > 6 {
        return "…".into();
    }
    let field = |key: &str| graph.child(node, key);
    match graph.class(node) {
        c if c.starts_with("CNmControlParameter") => graph
            .parameters
            .get(node)
            .cloned()
            .unwrap_or_else(|| format!("param#{node}")),
        "CNmConstFloatNode" => format!("{}", graph.number(node, "m_flValue").unwrap_or(0.0)),
        "CNmCachedFloatNode" | "CNmFloatSpringNode" | "CNmFloatCurveNode" => format!(
            "{}({})",
            graph
                .class(node)
                .trim_start_matches("CNm")
                .trim_end_matches("Node"),
            value(graph, field("m_nInputValueNodeIdx"), depth + 1)
        ),
        c => format!(
            "{}[{}]",
            c.trim_start_matches("CNm").trim_end_matches("Node"),
            node
        ),
    }
}

/// A condition node in words.
fn condition(graph: &Graph, node: Option<usize>, depth: usize) -> String {
    let Some(node) = node else {
        return "always".into();
    };
    if depth > 8 {
        return "…".into();
    }
    let raw = &graph.nodes[node];
    let strings = |key: &str| -> Vec<String> {
        raw.get(key)
            .and_then(ofast_core::kv3::Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect()
    };
    let sub = |key: &str| {
        graph
            .children(node, key)
            .into_iter()
            .map(|c| condition(graph, Some(c), depth + 1))
            .collect::<Vec<_>>()
    };
    let input = || value(graph, graph.child(node, "m_nInputValueNodeIdx"), depth + 1);
    match graph.class(node) {
        "CNmIDComparisonNode" => {
            let op = raw
                .get("m_comparison")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let not = if op == "DoesntMatch" { "not " } else { "" };
            format!("{} {not}in {:?}", input(), strings("m_comparisionIDs"))
        }
        "CNmAndNode" => format!("({})", sub("m_conditionNodeIndices").join(" AND ")),
        "CNmOrNode" => format!("({})", sub("m_conditionNodeIndices").join(" OR ")),
        "CNmNotNode" => format!(
            "NOT {}",
            condition(graph, graph.child(node, "m_nInputValueNodeIdx"), depth + 1)
        ),
        "CNmFloatComparisonNode" => {
            let op = raw
                .get("m_comparison")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let rhs = graph.child(node, "m_nComparandValueNodeIdx").map_or_else(
                || {
                    format!(
                        "{}",
                        graph.number(node, "m_flComparisonValue").unwrap_or(0.0)
                    )
                },
                |c| value(graph, Some(c), depth + 1),
            );
            format!("{} {op} {rhs}", input())
        }
        "CNmFloatRangeComparisonNode" => {
            let range = raw.get("m_range");
            let at = |k: &str| {
                range
                    .and_then(|r| r.get(k))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            };
            format!("{} in [{}, {}]", input(), at("m_flMin"), at("m_flMax"))
        }
        "CNmIDEventConditionNode" | "CNmGraphEventConditionNode" => {
            // ID events list `m_eventIDs`; graph events their rules' `m_conditions[].m_eventID`.
            let mut ids = strings("m_eventIDs");
            ids.extend(
                raw.get("m_conditions")
                    .and_then(ofast_core::kv3::Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|c| {
                        c.get("m_eventID")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    }),
            );
            format!("event {ids:?}")
        }
        "CNmStateCompletedConditionNode" => "state completed".into(),
        "CNmTimeConditionNode" => format!(
            "time {} {}",
            raw.get("m_operator")
                .and_then(|v| v.as_str())
                .unwrap_or("?"),
            graph.number(node, "m_flComparand").unwrap_or(0.0)
        ),
        c if c.starts_with("CNmControlParameter") => value(graph, Some(node), depth),
        c => format!(
            "{}[{node}]",
            c.trim_start_matches("CNm").trim_end_matches("Node")
        ),
    }
}

/// A state machine's states: entry condition, transitions, what each state plays; nested machines indented.
fn print_machine(graph: &Graph, machine: usize, depth: usize) {
    let indent = "  ".repeat(depth);
    let states = graph.states(machine);
    let definitions = graph.nodes[machine]
        .get("m_stateDefinitions")
        .and_then(ofast_core::kv3::Value::as_array)
        .unwrap_or_default();
    for (k, (state, entry)) in states.iter().enumerate() {
        println!(
            "{indent}[{k}] {} (node {state})  entry: {}",
            name(graph, *state),
            condition(graph, *entry, 0)
        );
        let transitions = definitions
            .get(k)
            .and_then(|d| d.get("m_transitionDefinitions"))
            .and_then(ofast_core::kv3::Value::as_array)
            .unwrap_or_default();
        for t in transitions {
            let target = t
                .get("m_nTargetStateIdx")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            let cond = t
                .get("m_nConditionNodeIdx")
                .and_then(|v| v.as_i64())
                .and_then(|i| usize::try_from(i).ok());
            let duration = t
                .get("m_nTransitionNodeIdx")
                .and_then(|v| v.as_i64())
                .and_then(|i| usize::try_from(i).ok())
                .and_then(|n| graph.number(n, "m_flDuration"))
                .unwrap_or(0.0);
            let target_name = usize::try_from(target)
                .ok()
                .and_then(|t| states.get(t))
                .map_or("?".into(), |(s, _)| name(graph, *s));
            println!(
                "{indent}    -> {target_name} ({duration} s) if {}",
                condition(graph, cond, 0)
            );
        }
        // What the state plays: nested machines, else its clips.
        let nested: Vec<usize> = (0..graph.paths.len())
            .filter(|&n| {
                graph.class(n) == "CNmStateMachineNode"
                    && graph.paths[n].starts_with(&format!("{}/", graph.paths[*state]))
                    && graph.paths[n].matches('/').count()
                        == graph.paths[*state].matches('/').count() + 1
            })
            .collect();
        if nested.is_empty() {
            let clips: Vec<String> = graph
                .clips_under(*state)
                .into_iter()
                .map(|(c, s)| {
                    let file = c
                        .rsplit('/')
                        .next()
                        .unwrap_or(&c)
                        .trim_end_matches(".vnmclip")
                        .to_owned();
                    if (s - 1.0).abs() > 1e-3 {
                        format!("{file}×{s}")
                    } else {
                        file
                    }
                })
                .collect();
            if !clips.is_empty() {
                println!("{indent}    plays {}", clips.join(", "));
            }
        }
        for m in nested {
            println!("{indent}  machine {}:", name(graph, m));
            print_machine(graph, m, depth + 2);
        }
    }
}
