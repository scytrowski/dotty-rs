use std::{fs, path::PathBuf, process::Command};

use dotty_tasty::tasty::*;
use serde_json::{Value, json};
use tasty_pickler_oracle::{MAX_FILE_BYTES, inspect, reduced_answer};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/tasty_pickler")
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(fixtures().join("tasty").join(name).with_extension("tasty")).unwrap()
}

fn nodes(snapshot: &Value) -> &[Value] {
    snapshot["nodes"].as_array().unwrap()
}

fn named<'a>(snapshot: &'a Value, name: &str) -> &'a Value {
    nodes(snapshot)
        .iter()
        .find(|n| n["name"] == json!({"utf8": name}))
        .unwrap()
}

#[test]
fn independent_fixtures_match_normalized_snapshots() {
    for name in [
        "Empty",
        "example/Answer",
        "example/Parameters",
        "example/Declarations",
        "example/References",
        "example/Operations",
        "example/Multi",
        "example/ParameterDecl",
        "example/Blocks",
        "example/Decisions",
        "example/Flags",
        "example/Conditionals",
        "example/Algorithms",
        "example/AlgorithmsReference",
        "example/PrivateValues",
        "example/MutableField",
        "example/Box",
        "example/Counter",
        "example/CustomOperators",
    ] {
        let expected: Value = serde_json::from_slice(
            &fs::read(
                fixtures()
                    .join("snapshots")
                    .join(name)
                    .with_extension("json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(inspect(&fixture(name), false).unwrap(), expected, "{name}");
    }
}

#[test]
fn scala_39_private_primitive_members_remain_source_valdefs_at_pickle_time() {
    let snapshot = inspect(&fixture("example/PrivateValues"), false).unwrap();
    let nodes = nodes(&snapshot);
    let class = named(&snapshot, "PrivateValues");
    let vals = ["seed", "enabled"].map(|name| named(&snapshot, name));
    assert_eq!(class["tag"], "TYPEDEF");
    let constructor = nodes
        .iter()
        .find(|node| node["tag"] == "DEFDEF" && node["name"] == json!({"utf8":"<init>"}))
        .unwrap();
    assert_eq!(constructor["role"], "compiler-default-constructor");
    assert_eq!(constructor["header"], json!([{"clause":"EMPTYCLAUSE"}]));
    assert_eq!(constructor["has_rhs"], false);
    assert_eq!(constructor["modifiers"], json!([]));
    assert_eq!(
        vals.map(|value| value["tag"].as_str().unwrap()),
        ["VALDEF", "VALDEF"]
    );
    for value in vals {
        assert_eq!(value["parent"], class["children"][0]);
        assert_eq!(value["owner"], class["id"]);
        assert_eq!(value["modifiers"], json!(["tag-6", "tag-22"]));
    }
    let template = nodes
        .iter()
        .find(|node| node["id"] == class["children"][0])
        .unwrap();
    assert_eq!(template["children"][2], vals[0]["id"]);
    assert_eq!(template["children"][3], vals[1]["id"]);
    assert_eq!(
        named(&snapshot, "seed")["children"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node["name"] == json!({"utf8":"seed"}))
            .count(),
        1
    );
    assert!(nodes.iter().any(|node| node["tag"] == "INTconst"));
    assert!(nodes.iter().any(|node| node["tag"] == "TRUEconst"));
    assert!(!nodes.iter().any(|node| node["tag"] == "DEFDEF"
        && matches!(node["name"]["utf8"].as_str(), Some("seed" | "enabled"))));
}

#[test]
fn scala_39_plain_constructor_parameter_has_template_and_constructor_param_nodes() {
    let snapshot = inspect(&fixture("example/Box"), false).unwrap();
    let nodes = nodes(&snapshot);
    let class = named(&snapshot, "Box");
    let template = nodes
        .iter()
        .find(|node| node["tag"] == "TEMPLATE" && node["parent"] == class["id"])
        .unwrap();
    let template_parameter = nodes
        .iter()
        .find(|node| {
            node["tag"] == "PARAM"
                && node["name"] == json!({"utf8":"start"})
                && node["parent"] == template["id"]
        })
        .unwrap();
    let constructor = nodes
        .iter()
        .find(|node| {
            node["tag"] == "DEFDEF"
                && node["name"] == json!({"utf8":"<init>"})
                && node["parent"] == template["id"]
        })
        .unwrap();
    let constructor_parameter = nodes
        .iter()
        .find(|node| {
            node["tag"] == "PARAM"
                && node["name"] == json!({"utf8":"start"})
                && node["parent"] == constructor["id"]
        })
        .unwrap();
    assert_ne!(template_parameter["id"], constructor_parameter["id"]);
    assert_eq!(template_parameter["owner"], class["id"]);
    assert_eq!(constructor_parameter["owner"], constructor["id"]);
    assert_eq!(constructor["modifiers"], json!(["STABLE"]));
    assert_eq!(
        constructor["header"],
        json!([{"parameter":{"utf8":"start"}}])
    );
    assert_eq!(template["children"][0], template_parameter["id"]);
    let parent_call = nodes
        .iter()
        .find(|node| node["id"] == template["children"][1])
        .unwrap();
    assert_eq!(parent_call["tag"], "APPLY");
    assert_eq!(template["children"][2], constructor["id"]);
    assert!(
        !nodes
            .iter()
            .any(|node| node["tag"] == "VALDEF" && node["name"] == json!({"utf8":"start"}))
    );
}

#[test]
fn scala_39_counter_tasty_keeps_initializer_owner_and_source_field_edges() {
    let snapshot = inspect(&fixture("example/Counter"), false).unwrap();
    let nodes = nodes(&snapshot);
    let class = named(&snapshot, "Counter");
    let template = nodes
        .iter()
        .find(|node| node["tag"] == "TEMPLATE" && node["parent"] == class["id"])
        .unwrap();
    let template_parameter = nodes
        .iter()
        .find(|node| {
            node["tag"] == "PARAM"
                && node["name"] == json!({"utf8":"start"})
                && node["parent"] == template["id"]
        })
        .unwrap();
    let constructor = nodes
        .iter()
        .find(|node| {
            node["tag"] == "DEFDEF"
                && node["name"] == json!({"utf8":"<init>"})
                && node["parent"] == template["id"]
        })
        .unwrap();
    let constructor_parameter = nodes
        .iter()
        .find(|node| {
            node["tag"] == "PARAM"
                && node["name"] == json!({"utf8":"start"})
                && node["parent"] == constructor["id"]
        })
        .unwrap();
    assert_ne!(template_parameter["id"], constructor_parameter["id"]);
    assert_eq!(
        constructor["header"],
        json!([{"parameter":{"utf8":"start"}}])
    );
    assert_eq!(constructor["modifiers"], json!([]));
    assert_eq!(template["children"][0], template_parameter["id"]);
    assert_eq!(template["children"][2], constructor["id"]);

    let current = named(&snapshot, "current");
    assert_eq!(current["tag"], "VALDEF");
    assert_eq!(current["owner"], class["id"]);
    assert_eq!(current["modifiers"], json!(["tag-6", "tag-22", "MUTABLE"]));
    assert_eq!(template["children"][3], current["id"]);
    let member_order = template["children"].as_array().unwrap()[3..]
        .iter()
        .map(|id| {
            let node = nodes.iter().find(|node| node["id"] == *id).unwrap();
            (node["tag"].as_str().unwrap(), node["name"]["utf8"].as_str())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        member_order,
        [
            ("VALDEF", Some("current")),
            ("DEFDEF", Some("get")),
            ("DEFDEF", Some("set")),
            ("DEFDEF", Some("increment")),
        ]
    );
    let initializer = current["children"][1].as_str().unwrap();
    let reference = nodes.iter().find(|node| node["id"] == initializer).unwrap();
    assert_eq!(reference["tag"], "TERMREFsymbol");
    assert_eq!(reference["reference"]["target"], template_parameter["id"]);
    assert_ne!(
        reference["reference"]["target"],
        constructor_parameter["id"]
    );

    let get = named(&snapshot, "get");
    let get_rhs = nodes
        .iter()
        .find(|node| {
            node["parent"] == get["id"]
                && node["tag"] == "SELECT"
                && node["name"] == json!({"utf8":"current"})
        })
        .unwrap();
    assert_eq!(
        nodes
            .iter()
            .find(|node| node["id"] == get_rhs["children"][0])
            .unwrap()["tag"],
        "QUALTHIS"
    );
    let set = named(&snapshot, "set");
    let assign = nodes
        .iter()
        .find(|node| node["parent"] == set["id"] && node["tag"] == "ASSIGN")
        .unwrap();
    let target = nodes
        .iter()
        .find(|node| node["id"] == assign["children"][0])
        .unwrap();
    assert_eq!(target["tag"], "SELECT");
    assert_eq!(target["name"], json!({"utf8":"current"}));
    let value = nodes
        .iter()
        .find(|node| node["id"] == assign["children"][1])
        .unwrap();
    assert_eq!(value["tag"], "TERMREFdirect");

    assert!(!nodes.iter().any(|node| node["tag"] == "DEFDEF"
        && matches!(node["name"]["utf8"].as_str(), Some("current" | "current_="))));
}

#[test]
fn scala_39_primitive_control_flow_fixture_pins_mutable_while_and_assignment_wire_edges() {
    let snapshot = inspect(&fixture("example/AlgorithmsReference"), false).unwrap();
    let sum_to = named(&snapshot, "sumTo");
    let nodes = nodes(&snapshot);
    let by_id = |id: &Value| {
        nodes
            .iter()
            .find(|node| node["id"] == *id)
            .unwrap_or_else(|| panic!("missing node {id}"))
    };
    let reference_target = |id: &Value| {
        let mut current = id.as_str().unwrap().to_owned();
        loop {
            let node = nodes
                .iter()
                .find(|node| node["id"].as_str() == Some(&current))
                .unwrap();
            if node["tag"] == "SHAREDtype" || node["tag"] == "SHAREDterm" {
                current = node["reference"]["target"].as_str().unwrap().to_owned();
                continue;
            }
            return node["reference"]["target"].clone();
        }
    };
    let loop_node = nodes
        .iter()
        .find(|node| node["tag"] == "WHILE" && node["owner"] == sum_to["id"])
        .unwrap();
    let condition = by_id(&loop_node["condition"]);
    assert_eq!(condition["tag"], "APPLY");
    let less = by_id(&condition["children"][0]);
    assert_eq!(less["tag"], "SELECTin");
    assert_eq!(less["name"]["signed"], json!({"utf8":"<"}));
    assert_eq!(
        less["name"]["parameters"],
        json!([{"qualified":[{"utf8":"scala"},{"utf8":"Int"}]}])
    );
    assert_eq!(
        less["name"]["result"],
        json!({"qualified":[{"utf8":"scala"},{"utf8":"Boolean"}]})
    );

    let block = by_id(&loop_node["body"]);
    assert_eq!(block["tag"], "BLOCK");
    // TASTy stores the block's final expression before its preceding stats;
    // evaluation order is stats first, then expression.
    let final_assignment = by_id(&block["expression"]);
    let preceding_assignment = by_id(&block["stats"][0]);
    assert_eq!(final_assignment["tag"], "ASSIGN");
    assert_eq!(preceding_assignment["tag"], "ASSIGN");
    assert_eq!(
        reference_target(&preceding_assignment["left"]),
        named(&snapshot, "total")["id"]
    );
    assert_eq!(
        reference_target(&final_assignment["left"]),
        named(&snapshot, "i")["id"]
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node["tag"] == "SELECTin"
                && node["owner"] == sum_to["id"]
                && node["name"]["signed"] == json!({"utf8":"+"}))
            .count(),
        2
    );
    for local_name in ["i", "total"] {
        let local = named(&snapshot, local_name);
        assert_eq!(local["tag"], "VALDEF");
        assert_eq!(local["modifiers"], json!(["MUTABLE"]));
        assert_eq!(local["owner"], sum_to["id"]);
    }
    assert!(nodes.iter().any(|node| node["tag"] == "VALDEF"
        && node["name"] == json!({"utf8":"saved"})
        && node["modifiers"] == json!([])));
}

#[test]
fn scala_control_flow_fixtures_pin_constants_if_order_blocks_and_local_identity() {
    let decisions = inspect(&fixture("example/Decisions"), false).unwrap();
    let unit = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "UNITconst")
        .unwrap();
    assert_eq!(unit["value"], "unit");
    let true_value = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "TRUEconst")
        .unwrap();
    let false_value = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "FALSEconst")
        .unwrap();
    assert_eq!(true_value["value"], true);
    assert_eq!(false_value["value"], false);

    let nested: Vec<_> = nodes(&decisions)
        .iter()
        .filter(|n| n["tag"] == "IF" && n["owner"] == named(&decisions, "nested")["id"])
        .collect();
    assert_eq!(nested.len(), 2);
    let outer = nested
        .iter()
        .find(|n| n["parent"] == named(&decisions, "nested")["id"])
        .unwrap();
    let inner = nested.iter().find(|n| n["parent"] == outer["id"]).unwrap();
    assert_eq!(outer["then"], inner["id"]);
    assert_eq!(
        outer["children"],
        json!([outer["condition"], outer["then"], outer["else"]])
    );

    let blocks = inspect(&fixture("example/Blocks"), false).unwrap();
    let block = nodes(&blocks).iter().find(|n| n["tag"] == "BLOCK").unwrap();
    let discarded_if = nodes(&blocks).iter().find(|n| n["tag"] == "IF").unwrap();
    assert_eq!(block["stats"], json!([discarded_if["id"]]));
    assert_eq!(
        block["children"],
        json!([block["expression"], discarded_if["id"]])
    );

    let stored_block = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "BLOCK" && n["owner"] == named(&decisions, "stored")["id"])
        .unwrap();
    let stored_if = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "IF" && n["parent"] == stored_block["expression"])
        .unwrap();
    let shared_expression = nodes(&decisions)
        .iter()
        .find(|n| n["id"] == stored_block["expression"])
        .unwrap();
    let saved = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "VALDEF" && n["name"] == json!({"utf8":"saved"}))
        .unwrap();
    assert_eq!(shared_expression["tag"], "TYPED");
    assert!(
        shared_expression["children"]
            .as_array()
            .unwrap()
            .contains(&stored_if["id"])
    );
    assert_eq!(stored_block["stats"], json!([saved["id"]]));
    let saved_reference = nodes(&decisions)
        .iter()
        .find(|n| n["tag"] == "TERMREFdirect" && n["reference"]["target"] == saved["id"])
        .unwrap();
    assert_eq!(saved_reference["id"], stored_if["then"]);
    assert_eq!(saved_reference["reference"]["direction"], "forward");
}

#[test]
fn answer_pins_owner_constructor_parent_return_and_real_literal() {
    let snapshot = inspect(&fixture("example/Answer"), false).unwrap();
    assert_eq!(snapshot["header"]["version"], json!([28, 9, 0]));
    let class = named(&snapshot, "Answer");
    let ctor = named(&snapshot, "<init>");
    let method = named(&snapshot, "value");
    assert_eq!(class["owner"], json!("n0"));
    assert_eq!(ctor["owner"], class["id"]);
    assert_eq!(ctor["header"], json!([{"clause": "EMPTYCLAUSE"}]));
    assert_eq!(ctor["modifiers"], json!(["STABLE"]));
    assert_eq!(ctor["has_rhs"], false);
    assert_eq!(method["owner"], class["id"]);
    assert_eq!(method["header"], json!([]));
    assert_eq!(method["has_rhs"], true);
    let literal = nodes(&snapshot)
        .iter()
        .find(|n| n["tag"] == "INTconst")
        .unwrap();
    assert_eq!(literal["value"], 42);
    assert_eq!(literal["parent"], method["id"]);
    assert_eq!(named(&snapshot, "Unit")["owner"], ctor["id"]);
    assert_eq!(named(&snapshot, "Int")["tag"], "IDENTtpt");
    let selection = nodes(&snapshot)
        .iter()
        .find(|n| n["tag"] == "SELECTin")
        .unwrap();
    assert_eq!(selection["name"]["parameters"], json!([]));
    assert_eq!(
        selection["name"]["result"],
        json!({"qualified": [
            {"qualified": [{"utf8":"java"}, {"utf8":"lang"}]}, {"utf8":"Object"}
        ]})
    );
    assert!(nodes(&snapshot).iter().any(|n| n["tag"] == "SHAREDtype"
        && n["reference"]["target"] == named(&snapshot, "Object")["id"]));
}

#[test]
fn parameter_order_and_owners_are_independent_of_names_and_empty_clauses() {
    let snapshot = inspect(&fixture("example/Parameters"), false).unwrap();
    let method = named(&snapshot, "pick");
    assert_eq!(
        method["header"],
        json!([
            {"parameter": {"utf8": "first"}}, {"parameter": {"utf8": "second"}}
        ])
    );
    let parameters: Vec<_> = nodes(&snapshot)
        .iter()
        .filter(|n| n["tag"] == "PARAM")
        .collect();
    assert_eq!(parameters.len(), 2);
    assert_eq!(parameters[0]["name"], json!({"utf8":"first"}));
    assert_eq!(parameters[1]["name"], json!({"utf8":"second"}));
    assert!(parameters.iter().all(|p| p["owner"] == method["id"]));
    assert_eq!(
        named(&snapshot, "zero")["header"],
        json!([{"clause":"EMPTYCLAUSE"}])
    );
    let parameter_ref = nodes(&snapshot)
        .iter()
        .find(|n| n["tag"] == "TERMREFdirect")
        .unwrap();
    assert_eq!(parameter_ref["reference"]["target"], parameters[1]["id"]);
}

#[test]
fn parameter_decl_pins_literal_rhs_and_ordered_int_parameter_trees() {
    let snapshot = inspect(&fixture("example/ParameterDecl"), false).unwrap();
    let method = named(&snapshot, "constant");
    let parameters = nodes(&snapshot)
        .iter()
        .filter(|node| node["tag"] == "PARAM")
        .collect::<Vec<_>>();
    assert_eq!(parameters.len(), 2);
    assert_eq!(
        parameters
            .iter()
            .map(|parameter| parameter["name"]["utf8"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        parameters
            .iter()
            .all(|parameter| parameter["owner"] == method["id"])
    );
    assert!(parameters.iter().all(|parameter| {
        nodes(&snapshot).iter().any(|node| {
            node["id"] == parameter["children"][0]
                && node["tag"] == "IDENTtpt"
                && node["name"] == json!({"utf8":"Int"})
        })
    }));
    assert!(nodes(&snapshot).iter().any(|node| {
        node["tag"] == "INTconst" && node["parent"] == method["id"] && node["value"] == 42
    }));
}

#[test]
fn absent_abstract_rhs_is_distinct_from_an_untyped_concrete_rhs() {
    let snapshot = inspect(&fixture("example/Declarations"), false).unwrap();
    assert_eq!(
        named(&snapshot, "Declarations")["modifiers"],
        json!(["ABSTRACT", "ANNOTATION"])
    );
    assert_eq!(named(&snapshot, "missing")["has_rhs"], false);
    assert_eq!(named(&snapshot, "missing")["header"], json!([]));
    assert_eq!(named(&snapshot, "explicit")["has_rhs"], false);
    assert_eq!(
        named(&snapshot, "explicit")["header"],
        json!([{"clause":"EMPTYCLAUSE"}])
    );
}

#[test]
fn source_forward_method_selection_is_named_but_class_reference_has_identity() {
    let snapshot = inspect(&fixture("example/References"), false).unwrap();
    let selection = nodes(&snapshot)
        .iter()
        .find(|n| n["tag"] == "SELECT")
        .unwrap();
    assert_eq!(selection["name"], json!({"utf8":"second"}));
    let class_ref = nodes(&snapshot)
        .iter()
        .find(|n| n["tag"] == "TYPEREFsymbol")
        .unwrap();
    assert_eq!(
        class_ref["reference"]["target"],
        named(&snapshot, "References")["id"]
    );
    assert_eq!(class_ref["reference"]["direction"], "backward");
}

#[test]
fn operations_pins_calls_parameter_refs_forward_refs_and_clause_shapes() {
    let snapshot = inspect(&fixture("example/Operations"), false).unwrap();
    let method = |name: &str| {
        let found = nodes(&snapshot)
            .iter()
            .find(|node| node["tag"] == "DEFDEF" && node["name"] == json!({"utf8":name}))
            .unwrap();
        assert_eq!(found["owner"], named(&snapshot, "Operations")["id"]);
        found
    };
    assert_eq!(method("use")["header"], json!([]));
    assert_eq!(method("first")["header"], json!([]));
    assert_eq!(method("second")["header"], json!([]));
    assert_eq!(
        method("identity")["header"],
        json!([{"parameter":{"utf8":"x"}}])
    );
    assert_eq!(method("zero")["header"], json!([{"clause":"EMPTYCLAUSE"}]));

    let parameter = nodes(&snapshot)
        .iter()
        .find(|node| node["tag"] == "PARAM" && node["name"] == json!({"utf8":"x"}))
        .unwrap();
    assert_eq!(parameter["owner"], method("identity")["id"]);
    let parameter_ref = nodes(&snapshot)
        .iter()
        .find(|node| node["tag"] == "TERMREFdirect")
        .unwrap();
    assert_eq!(parameter_ref["reference"]["target"], parameter["id"]);

    let call = nodes(&snapshot)
        .iter()
        .find(|node| node["tag"] == "APPLY" && node["parent"] == method("use")["id"])
        .unwrap();
    let children = call["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    let function = nodes(&snapshot)
        .iter()
        .find(|node| node["id"] == children[0])
        .unwrap();
    assert_eq!(function["tag"], "SELECTin");
    assert_eq!(function["name"]["signed"], json!({"utf8":"identity"}));
    let qualifier = nodes(&snapshot)
        .iter()
        .find(|node| node["id"] == function["children"][0])
        .unwrap();
    assert_eq!(qualifier["tag"], "QUALTHIS");
    assert_eq!(
        named(&snapshot, "Operations")["id"],
        nodes(&snapshot)
            .iter()
            .find(|node| {
                node["tag"] == "TYPEREFsymbol" && node["parent"] == qualifier["children"][0]
            })
            .unwrap()["reference"]["target"]
    );
    let argument = nodes(&snapshot)
        .iter()
        .find(|node| node["id"] == children[1])
        .unwrap();
    assert_eq!(
        (argument["tag"].as_str(), argument["value"].as_i64()),
        (Some("INTconst"), Some(42))
    );

    // The forward source method reference is emitted as a named SELECT whose
    // qualifier is the canonical class `this`; it is not a forward AST address.
    let forward = nodes(&snapshot)
        .iter()
        .find(|node| node["tag"] == "SELECT" && node["name"] == json!({"utf8":"second"}))
        .unwrap();
    assert_eq!(forward["parent"], method("first")["id"]);
    let forward_qualifier = nodes(&snapshot)
        .iter()
        .find(|node| node["id"] == forward["children"][0])
        .unwrap();
    assert_eq!(forward_qualifier["tag"], "QUALTHIS");
    assert_eq!(method("second")["parent"], method("first")["parent"]);
}

#[test]
fn multi_pins_ordered_literal_methods_and_empty_value_clause() {
    let snapshot = inspect(&fixture("example/Multi"), false).unwrap();
    let class = named(&snapshot, "Multi");
    let methods = ["one", "two", "zero"].map(|name| {
        nodes(&snapshot)
            .iter()
            .find(|node| node["tag"] == "DEFDEF" && node["name"] == json!({"utf8":name}))
            .unwrap()
    });
    assert!(methods.iter().all(|method| method["owner"] == class["id"]));
    assert_eq!(methods[0]["header"], json!([]));
    assert_eq!(methods[1]["header"], json!([]));
    assert_eq!(methods[2]["header"], json!([{"clause":"EMPTYCLAUSE"}]));
    assert!(
        methods
            .iter()
            .all(|method| method["has_rhs"].as_bool() == Some(true))
    );
    let literals = nodes(&snapshot)
        .iter()
        .filter(|node| node["tag"] == "INTconst")
        .map(|node| (node["parent"].clone(), node["value"].as_i64().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(
        literals,
        methods
            .iter()
            .zip([1, 2, 0])
            .map(|(method, value)| (method["id"].clone(), value))
            .collect::<Vec<_>>()
    );
}

#[test]
fn uuid_source_path_and_section_order_are_normalized_without_losing_semantics() {
    let bytes = fixture("example/Answer");
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes).unwrap();
    let source = file
        .section(StandardSection::Attributes)
        .unwrap()
        .decode_attributes()
        .unwrap()
        .into_iter()
        .find_map(|a| match a {
            Attribute::SourceFile(r) => Some(r),
            _ => None,
        })
        .unwrap();
    let mut names = file.names().entries().to_vec();
    names[source as usize] = RawName::Utf8("different/source/path/Answer.scala".into());
    let mut header = file.header().clone();
    header.uuid = [99; 16];
    let mut builder = TastyFileBuilder::new(header, NameTable::from_entries(names).unwrap());
    for section in file.sections().entries().iter().rev() {
        builder.push_section(EncodedSection::raw(section.name, section.payload));
    }
    let changed = builder.encode_validated().unwrap();
    assert_ne!(changed, bytes);
    assert_eq!(
        inspect(&bytes, false).unwrap(),
        inspect(&changed, false).unwrap()
    );
    let raw = inspect(&bytes, true).unwrap();
    assert_eq!(raw["nodes"][0]["wire_address"], 0);
    assert!(
        inspect(&bytes, false).unwrap()["nodes"][0]
            .get("wire_address")
            .is_none()
    );
}

#[test]
fn a_string_literal_sharing_the_source_name_entry_is_not_normalized_away() {
    // A wire-only derivative exercises the normalizer, not Scala typing: replace
    // the Int leaf by a string referencing the very same name as SOURCEFILEattr.
    let bytes = fixture("example/Answer");
    let file = TastyFile::parse_and_validate_scala_3_9(&bytes).unwrap();
    let source = file
        .section(StandardSection::Attributes)
        .unwrap()
        .decode_attributes()
        .unwrap()
        .into_iter()
        .find_map(|a| match a {
            Attribute::SourceFile(r) => Some(r),
            _ => None,
        })
        .unwrap();
    let literal = file.ast_nodes_with_tag(INTCONST_TAG).unwrap()[0];
    let mut replacement = Writer::new();
    RawTree::leaf(STRINGCONST_TAG, TermValue::NameRef(source))
        .unwrap()
        .encode(&mut replacement)
        .unwrap();
    let replacement = replacement.into_inner();
    assert_eq!(replacement.len(), 2); // Same leaf width: no address relocation.
    let mut asts = file
        .section(StandardSection::Asts)
        .unwrap()
        .payload
        .to_vec();
    asts[literal.offset..literal.offset + replacement.len()].copy_from_slice(&replacement);
    let mut builder = TastyFileBuilder::new(file.header().clone(), file.names().clone());
    for section in file.sections().iter() {
        builder.push_section(EncodedSection::raw(
            section.name,
            if section.standard_kind(file.names()) == Some(StandardSection::Asts) {
                asts.as_slice()
            } else {
                section.payload
            },
        ));
    }
    let changed = builder.encode_validated().unwrap();
    let snapshot = inspect(&changed, false).unwrap();
    let literal = nodes(&snapshot)
        .iter()
        .find(|n| n["tag"] == "STRINGconst" && n["owner"] == named(&snapshot, "value")["id"])
        .unwrap();
    assert_eq!(
        literal["name"],
        json!({"utf8": file.render_name(source).unwrap()})
    );
}

#[test]
fn metadata_reduction_preserves_body_identity_and_rejects_other_fixture_shapes() {
    let original = fixture("example/Answer");
    let reduced = reduced_answer(&original).unwrap();
    let snapshot = inspect(&reduced, false).unwrap();
    assert_eq!(snapshot["sections"], json!(["ASTs", "Attributes"]));
    assert_eq!(named(&snapshot, "Answer")["modifiers"], json!([]));
    assert_eq!(named(&snapshot, "value")["has_rhs"], true);
    assert!(
        nodes(&snapshot)
            .iter()
            .any(|n| n["tag"] == "INTconst" && n["value"] == 42)
    );
    assert!(reduced.len() < original.len());
    assert!(reduced_answer(&fixture("Empty")).is_err());
    assert!(reduced_answer(&reduced).is_err());
}

#[test]
fn invalid_wire_and_excessive_size_fail_instead_of_producing_snapshots() {
    let bytes = fixture("example/Answer");
    let mut corrupt = bytes.clone();
    corrupt[0] ^= 1;
    assert!(
        inspect(&corrupt, false)
            .unwrap_err()
            .to_string()
            .contains("magic")
    );
    assert!(inspect(&bytes[..bytes.len() - 1], false).is_err());
    assert!(
        inspect(&vec![0; MAX_FILE_BYTES + 1], false)
            .unwrap_err()
            .to_string()
            .contains("1 MiB")
    );
}

#[test]
fn cli_rejects_malformed_candidate_and_unknown_arguments() {
    let binary = env!("CARGO_BIN_EXE_tasty-pickler-oracle");
    assert!(
        !Command::new(binary)
            .arg("unknown")
            .output()
            .unwrap()
            .status
            .success()
    );
    let result = Command::new(binary)
        .args(["inspect", "Cargo.toml"])
        .current_dir(fixtures().join("../../.."))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("magic"));
}
