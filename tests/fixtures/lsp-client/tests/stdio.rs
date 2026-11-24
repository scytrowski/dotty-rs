use lsp_server::{Message, Notification, Request};
use serde_json::{Value, json};
use std::{
    io::BufReader,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const A: &str = "file:///fixture/A.scala";
const B: &str = "file:///fixture/B.scala";
const TIMEOUT: Duration = Duration::from_secs(10);
const INT_METHOD: &str = "  def take(n: Int): Int = n";
const BOOL_METHOD: &str = "  def take(b: Boolean): Int = 7";

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    incoming: Receiver<Message>,
    next_id: i32,
    initially_clean: Vec<&'static str>,
}

impl Client {
    fn start(encoding: &str, initially_clean: Vec<&'static str>) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_dotty-lsp"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the fixture");
        let stdin = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, incoming) = mpsc::channel();
        thread::spawn(move || {
            while let Some(message) = Message::read(&mut stdout).expect("read LSP message") {
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            stdin,
            incoming,
            next_id: 1,
            initially_clean,
        };
        let init = client.request(
            "initialize",
            json!({
                "processId": null, "rootUri": null,
                "capabilities": {
                    "general": {"positionEncodings": [encoding]},
                    "textDocument": {
                        "definition": {"linkSupport": true},
                        "publishDiagnostics": {"versionSupport": true}
                    }
                }
            }),
        );
        assert_eq!(init["capabilities"]["positionEncoding"], encoding);
        assert_eq!(init["capabilities"]["definitionProvider"], true);
        assert_eq!(init["capabilities"]["referencesProvider"], true);
        assert_eq!(init["capabilities"]["textDocumentSync"]["change"], 2);
        client.notify("initialized", json!({}));
        client
    }

    fn receive(&self) -> Message {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .expect("LSP timeout");
            let message = self
                .incoming
                .recv_timeout(remaining)
                .expect("expected an LSP message");
            // A rebuild can republish clean documents opened at version one.
            if let Message::Notification(n) = &message
                && n.method == "textDocument/publishDiagnostics"
                && n.params["version"] == 1
                && n.params["uri"]
                    .as_str()
                    .is_some_and(|uri| self.initially_clean.contains(&uri))
            {
                assert_eq!(n.params["diagnostics"], json!([]));
                continue;
            }
            return message;
        }
    }

    fn send(&mut self, msg: impl Into<Message>) {
        msg.into().write(self.stdin.as_mut().unwrap()).unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = lsp_server::RequestId::from(self.next_id);
        self.next_id += 1;
        self.send(Request::new(id.clone(), method.into(), params));
        let Message::Response(reply) = self.receive() else {
            panic!("expected response to {method}")
        };
        assert_eq!(reply.id, id);
        assert!(reply.error.is_none(), "{method}: {:?}", reply.error);
        reply.result.unwrap_or(Value::Null)
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(Notification::new(method.into(), params));
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": uri, "version": 1, "languageId":"scala", "text":text
            }}),
        );
    }

    fn edit(&mut self, uri: &str, version: i32, range: Value, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"range":range, "text":text}]
            }),
        );
    }

    fn diagnostics(&self, uri: &str, version: i32, diagnostics: Value) {
        let Message::Notification(n) = self.receive() else {
            panic!("expected diagnostics for {uri} version {version}")
        };
        assert_eq!(n.method, "textDocument/publishDiagnostics");
        assert_eq!(n.params["uri"], uri);
        assert_eq!(n.params["version"], version);
        assert_eq!(
            n.params["diagnostics"], diagnostics,
            "{uri} version {version}"
        );
    }

    fn references(
        &mut self,
        uri: &str,
        line: usize,
        column: usize,
        include_declaration: bool,
    ) -> Value {
        self.request(
            "textDocument/references",
            json!({
                "textDocument":{"uri":uri}, "position":{"line":line,"character":column},
                "context":{"includeDeclaration":include_declaration}
            }),
        )
    }

    fn definition(&mut self, origin: usize, target_line: usize, declaration: &str) {
        let reply = self.request(
            "textDocument/definition",
            json!({
                "textDocument":{"uri": B}, "position":{"line":1, "character":origin + 1}
            }),
        );
        assert_eq!(
            reply,
            json!([{
                "originSelectionRange":range(1, origin, origin + 4),
                "targetUri": A,
                "targetSelectionRange":range(target_line, 6, 10),
                "targetRange":range(target_line, 2, declaration.len())
            }])
        );
    }

    fn shutdown(mut self) {
        assert_eq!(self.request("shutdown", Value::Null), Value::Null);
        self.notify("exit", Value::Null);
        drop(self.stdin.take());
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(Instant::now() < deadline, "server did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Reap the process even when a protocol assertion panics.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn range(line: usize, start: usize, end: usize) -> Value {
    json!({"start":{"line":line, "character":start}, "end":{"line":line, "character":end}})
}

fn exercise(encoding: &str, take_column: usize) {
    let mut client = Client::start(encoding, vec![A]);
    client.open(
        A,
        &format!("class A {{\r\n{INT_METHOD}\r\n{BOOL_METHOD}\r\n}}\r\n"),
    );
    client.open(
        B,
        concat!(
            "class B {\r\n",
            "  def good(a: A): Int = /* 😀 */ a.take(42)\r\n",
            "  def only(n: Int): Int = n\r\n",
            "  def bad(): Int = only(1)\r\n",
            "  def keep(a: A): Int = a.take(9)\r\n",
            "}\r\n"
        ),
    );
    client.diagnostics(B, 1, json!([]));
    client.definition(take_column, 1, INT_METHOD);

    let caller = json!({"uri": B, "range":range(1,take_column,take_column+4)});
    let stable_start = "  def keep(a: A): Int = a.".len();
    let stable = json!({"uri": B, "range":range(4,stable_start,stable_start+4)});
    let boolean = json!({"uri": A, "range":range(2,6,10)});
    let integer = json!({"uri": A, "range":range(1,6,10)});
    assert_eq!(
        client.references(B, 1, take_column + 1, false),
        json!([caller, stable])
    );
    assert_eq!(
        client.references(A, 1, 7, true),
        json!([integer, caller, stable])
    );
    assert_eq!(client.references(A, 2, 7, false), json!([]));
    assert_eq!(client.references(B, 1, take_column + 4, true), Value::Null);

    client.edit(B, 2, range(1, take_column + 5, take_column + 7), "false");
    client.diagnostics(B, 2, json!([]));
    client.definition(take_column, 2, BOOL_METHOD);
    assert_eq!(client.references(A, 1, 7, false), json!([stable]));
    assert_eq!(
        client.references(B, 1, take_column + 1, true),
        json!([boolean, caller])
    );

    client.edit(B, 3, range(3, 24, 25), "false");
    client.diagnostics(
        B,
        3,
        json!([{
            "range":range(3, 24, 29), "severity":1, "source":"dotty",
            "code":"typer.application_argument_type_mismatch", "message":"Expected Int, found Boolean"
        }]),
    );
    client.definition(take_column, 2, BOOL_METHOD);
    assert_eq!(client.references(A, 1, 7, false), json!([stable]));
    assert_eq!(
        client.references(B, 1, take_column + 1, true),
        json!([boolean, caller])
    );

    let only_position = json!({"textDocument": {"uri": B}, "position": {"line":3, "character":20}});
    assert_eq!(
        client.request("textDocument/definition", only_position.clone()),
        Value::Null
    );
    client.edit(B, 4, range(3, 24, 29), "1");
    client.diagnostics(B, 4, json!([]));
    client.definition(take_column, 2, BOOL_METHOD);
    assert_eq!(
        client.request("textDocument/definition", only_position),
        json!([{ "originSelectionRange":range(3,19,23), "targetUri": B,
            "targetSelectionRange":range(2,6,10), "targetRange":range(2,2,"  def only(n: Int): Int = n".len()) }])
    );
    client.shutdown();
}

#[test]
fn overload_navigation_and_diagnostics_utf8() {
    exercise("utf-8", 37);
}

#[test]
fn overload_navigation_and_diagnostics_utf16() {
    exercise("utf-16", 35);
}

#[test]
fn inferred_field_reports_one_call_error_and_clears_it_in_either_file_order() {
    let initial = "class Consumer {}";
    let broken = "class Consumer(provider: Provider) { val answer = provider.value(1) }";
    let provider_text = "class Provider { def value() = 40 }";
    let start = broken.find("provider.value(1)").unwrap();

    // Opening order and URI sort order both change, so analysis also visits the files in reverse.
    for provider_first in [false, true] {
        let (provider, consumer) = if provider_first {
            (
                "file:///first/Provider.scala",
                "file:///second/Consumer.scala",
            )
        } else {
            (
                "file:///second/Provider.scala",
                "file:///first/Consumer.scala",
            )
        };
        let mut client = Client::start("utf-16", vec![provider, consumer]);
        if provider_first {
            client.open(provider, provider_text);
            client.open(consumer, initial);
        } else {
            client.open(consumer, initial);
            client.open(provider, provider_text);
        }
        client.edit(consumer, 2, range(0, 0, initial.len()), broken);
        client.diagnostics(
            consumer,
            2,
            json!([{
                "range": range(0, start, start + "provider.value(1)".len()),
                "severity":1, "source":"dotty", "code":"typer.application_arity_mismatch", "message":"Expected 0 arguments, got 1"
            }]),
        );
        let arg = start + "provider.value(".len();
        client.edit(consumer, 3, range(0, arg, arg + 1), "");
        client.diagnostics(consumer, 3, json!([]));
        client.shutdown();
    }
}

#[test]
fn parent_argument_diagnostic_tracks_an_unsaved_type_edit_in_either_file_order() {
    let initial = "class Derived(start: Boolean)";
    let parent_clause = " extends Base(start)";
    let base_text = "class Base(seed: Int)";
    // The URI order controls analysis order, independently of notification timing.
    for (base, derived) in [
        ("file:///first/Base.scala", "file:///second/Derived.scala"),
        ("file:///second/Base.scala", "file:///first/Derived.scala"),
    ] {
        let mut client = Client::start("utf-16", vec![base, derived]);
        let mut docs = [(base, base_text), (derived, initial)];
        docs.sort_by_key(|(uri, _)| *uri);
        for (uri, text) in docs {
            client.open(uri, text);
        }
        client.edit(
            derived,
            2,
            range(0, initial.len(), initial.len()),
            parent_clause,
        );
        let argument = initial.len() + parent_clause.find("start").unwrap();
        client.diagnostics(
            derived,
            2,
            json!([{
                "range": range(0, argument, argument + "start".len()),
                "severity": 1, "source": "dotty", "code":"typer.application_argument_type_mismatch", "message": "Expected Int, found Boolean"
            }]),
        );
        let annotation = initial.find("Boolean").unwrap();
        client.edit(
            derived,
            3,
            range(0, annotation, annotation + "Boolean".len()),
            "Int",
        );
        client.diagnostics(derived, 3, json!([]));
        client.shutdown();
    }
}

#[test]
fn repairing_an_unknown_type_clears_the_marker_and_preserves_overload_navigation() {
    let uri = "file:///fixture/Broken.scala";
    let broken = "class Broken(/* 😀 */ start: Missing)";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A, B]);
        client.open(A, &format!("class A {{\n{INT_METHOD}\n{BOOL_METHOD}\n}}"));
        let caller = "  def good(a: A): Int = a.take(false)";
        client.open(B, &format!("class B {{\n{caller}\n}}"));
        client.open(uri, broken);
        let prefix = broken.split_once("Missing").unwrap().0;
        let start = if encoding == "utf-8" {
            prefix.len()
        } else {
            prefix.encode_utf16().count()
        };
        let location = range(0, start, start + "Missing".len());
        let unknown = json!([{"range":location,"severity":1,"source":"dotty","code":"typer.type_name_not_found","message":"Unknown type name"}]);
        client.diagnostics(uri, 1, unknown.clone());
        let child = "file:///fixture/Child.scala";
        let child_text = format!("class Child(start: Int) extends Broken(start) {{\n{caller}\n}}");
        client.open(child, &child_text);
        client.diagnostics(uri, 1, unknown);
        client.diagnostics(child, 1, json!([]));
        let column = caller.find("take").unwrap();
        let position =
            json!({"textDocument":{"uri":child},"position":{"line":1,"character":column + 1}});
        assert_eq!(
            client.request("textDocument/definition", position.clone()),
            Value::Null
        );
        client.definition(column, 2, BOOL_METHOD);

        client.edit(uri, 2, location.clone(), "Int");
        client.diagnostics(uri, 2, json!([]));
        client.diagnostics(child, 1, json!([]));
        let resolved = client.request("textDocument/definition", position.clone());
        assert_eq!(
            resolved,
            json!([{
                "originSelectionRange":range(1,column,column+4),
                "targetUri":A, "targetSelectionRange":range(2,6,10),
                "targetRange":range(2,2,BOOL_METHOD.len())
            }])
        );
        client.definition(column, 2, BOOL_METHOD);

        client.edit(uri, 3, range(0, start, start + 3), "Missing");
        client.diagnostics(
            uri,
            3,
            json!([{"range":location,"severity":1,"source":"dotty",
            "code":"typer.type_name_not_found","message":"Unknown type name"}]),
        );
        client.diagnostics(child, 1, json!([]));
        assert_eq!(
            client.request("textDocument/definition", position),
            Value::Null
        );
        client.definition(column, 2, BOOL_METHOD);
        client.shutdown();
    }
}

#[test]
fn adding_and_repairing_a_trait_keeps_a_later_class_navigable() {
    let prefix = "/* 😀 */ ";
    for (encoding, column) in [
        ("utf-8", prefix.len()),
        ("utf-16", prefix.encode_utf16().count()),
    ] {
        let mut client = Client::start(encoding, vec![A, B]);
        client.open(
            A,
            &format!("class First\n{prefix}\nclass A {{\n{INT_METHOD}\n{BOOL_METHOD}\n}}"),
        );
        let caller = "  def read(a: A): Int = a.take(false)";
        client.open(B, &format!("class B {{\n{caller}\n}}"));
        let take = caller.find("take").unwrap();
        client.definition(take, 4, BOOL_METHOD);

        client.edit(A, 2, range(1, column, column), "trait T");
        client.diagnostics(A, 2, json!([{
            "source": "dotty", "severity": 1, "range": range(1, column, column + 7),
            "code": "declaration.unsupported", "message": "Unsupported declaration in the ordinary-class view"
        }]));
        client.definition(take, 4, BOOL_METHOD);

        client.edit(A, 3, range(1, column, column + 5), "class");
        client.diagnostics(A, 3, json!([]));
        client.definition(take, 4, BOOL_METHOD);
        client.shutdown();
    }
}

#[test]
fn parser_then_namer_diagnostics_track_unsaved_edits_in_both_encodings() {
    // This document sorts between A and B. While it is rejected, B is the
    // third open document but only the second file in the semantic batch.
    let uri = "file:///fixture/ARejected.scala";
    for (encoding, literal, qualifier) in [("utf-8", 46, 21), ("utf-16", 44, 19)] {
        let mut client = Client::start(encoding, vec![A, B]);
        client.open(A, &format!("class A {{\n{INT_METHOD}\n{BOOL_METHOD}\n}}"));
        let caller_line = "  def good(a: A): Int = /* 😀 */ a.take(false)";
        client.open(B, &format!("class B {{\n{caller_line}\n}}"));
        let take = if encoding == "utf-8" { 37 } else { 35 };
        client.open(
            uri,
            "class Outer {\n  /* 😀 */ private[Missing] val hidden: Int 1\n}",
        );
        client.diagnostics(
            uri,
            1,
            json!([
                {
                    "source": "dotty", "severity": 1, "range": range(1, literal, literal + 1),
                    "code": "parser.declaration.expected_value_equals",
                    "message": "Expected '=' before the initializer"
                }
            ]),
        );
        client.definition(take, 2, BOOL_METHOD);
        client.edit(uri, 2, range(1, literal, literal), "= ");
        client.diagnostics(uri, 2, json!([{
            "source": "dotty", "severity": 1, "range": range(1, qualifier, qualifier + 7),
            "code": "namer.visibility.invalid_qualifier",
            "message": "Invalid 'private' qualifier 'Missing': expected an enclosing class, object or package"
        }]));
        client.definition(take, 2, BOOL_METHOD);
        client.edit(uri, 3, range(1, qualifier - 1, qualifier + 8), "");
        client.diagnostics(uri, 3, json!([]));
        client.definition(take, 2, BOOL_METHOD);
        // A separate, complete declaration on the same line still needs a separator.
        client.edit(uri, 4, range(2, 0, 0), "  val next = 2 val tail = 3\n");
        client.diagnostics(
            uri,
            4,
            json!([{
                "source": "dotty", "severity": 1, "range": range(2, 15, 18),
                "code": "parser.template.expected_member_separator",
                "message": "Parser diagnostic (parser.template.expected_member_separator)"
            }]),
        );
        client.edit(uri, 5, range(2, 15, 15), "; ");
        client.diagnostics(uri, 5, json!([]));
        client.shutdown();
    }
}

#[test]
fn a_document_recovers_navigation_when_preparation_first_yields_classes() {
    let caller = "  def good(): Int = a.take(false)";
    let header = "class B(a: A) {";
    let take = caller.find("take").unwrap();
    let lookup = json!({"textDocument":{"uri":B}, "position":{"line":1,"character":take+1}});
    let unknown = header.find('A').unwrap();
    let missing_class = json!([{"source":"dotty", "severity":1, "range":range(0,unknown,unknown+1),
        "code":"typer.type_name_not_found", "message":"Unknown type name"}]);
    for (encoding, literal, qualifier, trait_col) in [("utf-8", 46, 21, 11), ("utf-16", 44, 19, 9)]
    {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &format!("class A {{\n{INT_METHOD}\n{BOOL_METHOD}\n}}"));
        client.open(B, &format!("{header}\n{caller}\n}}"));
        client.diagnostics(B, 1, json!([]));
        client.definition(take, 2, BOOL_METHOD);

        client.edit(
            A,
            2,
            range(3, 1, 1),
            "\n/* 😀 */ trait T\nclass Outer {\n  /* 😀 */ private[Missing] val hidden: Int 1\n}",
        );
        client.diagnostics(A, 2, json!([{"source":"dotty","severity":1,"range":range(6,literal,literal+1),
            "code":"parser.declaration.expected_value_equals","message":"Expected '=' before the initializer"}]));
        client.diagnostics(B, 1, missing_class.clone());
        assert_eq!(
            client.request("textDocument/definition", lookup.clone()),
            Value::Null
        );

        client.edit(A, 3, range(6, literal, literal), "= ");
        client.diagnostics(A, 3, json!([{"source":"dotty","severity":1,"range":range(6,qualifier,qualifier+7),
            "code":"namer.visibility.invalid_qualifier","message":"Invalid 'private' qualifier 'Missing': expected an enclosing class, object or package"}]));
        client.diagnostics(B, 1, missing_class.clone());
        assert_eq!(
            client.request("textDocument/definition", lookup.clone()),
            Value::Null
        );

        client.edit(A, 4, range(6, qualifier - 1, qualifier + 8), "");
        client.diagnostics(A, 4, json!([{"source":"dotty","severity":1,"range":range(4,trait_col,trait_col+7),
            "code":"declaration.unsupported","message":"Unsupported declaration in the ordinary-class view"}]));
        client.diagnostics(B, 1, json!([]));
        client.definition(take, 2, BOOL_METHOD);

        client.edit(A, 5, range(4, trait_col, trait_col + 5), "class");
        client.diagnostics(A, 5, json!([]));
        client.diagnostics(B, 1, json!([]));
        client.definition(take, 2, BOOL_METHOD);
        client.shutdown();
    }
}

#[test]
fn lexer_and_parser_markers_clear_independently_after_unsaved_edits() {
    let uri = "file:///fixture/Broken.scala";
    for (encoding, literal, comment) in [("utf-8", 42, 46), ("utf-16", 40, 44)] {
        let mut client = Client::start(encoding, vec![]);
        let parser = json!({
            "source":"dotty", "severity":1, "range": range(0, literal, literal + 1),
            "code":"parser.declaration.expected_value_equals", "message":"Expected '=' before the initializer"
        });
        let unterminated = |start| {
            json!({
                "source":"dotty", "severity":1, "range": range(0, start, start + "/* open".len()),
                "code":"lexer.unterminated_block_comment", "message":"Unterminated block comment"
            })
        };
        client.open(uri, "/* 😀 */ class Broken { val hidden: Int 1 } /* open");
        client.diagnostics(uri, 1, json!([parser, unterminated(comment)]));
        let end = comment + "/* open".len();
        client.edit(uri, 2, range(0, end, end), "*/");
        client.diagnostics(uri, 2, json!([parser]));
        client.edit(uri, 3, range(0, literal, literal), "= ");
        client.diagnostics(uri, 3, json!([]));

        // Reopen the trailing comment with otherwise valid syntax.
        client.edit(uri, 4, range(0, end + 2, end + 4), "");
        client.diagnostics(uri, 4, json!([unterminated(comment + 2)]));
        client.shutdown();
    }
}

#[test]
fn numeric_edits_update_the_message_and_range_for_the_same_code() {
    let uri = "file:///fixture/A.scala";
    for (encoding, start) in [("utf-8", 40), ("utf-16", 38)] {
        let mut client = Client::start(encoding, vec![]);
        let marker = |len, message: &str| {
            json!([{
                "source": "dotty", "severity": 1, "range": range(1, start, start + len),
                "code": "lexer.numeric.missing_digits", "message": message
            }])
        };
        client.open(
            uri,
            "package demo\n/* 😀 */ class A { def value(): Int = 0x }",
        );
        client.diagnostics(
            uri,
            1,
            marker(2, "Expected a base-16 digit after the prefix"),
        );
        client.edit(uri, 2, range(1, start, start + 2), "1e+");
        client.diagnostics(uri, 2, marker(3, "Expected a digit in the exponent"));
        client.edit(uri, 3, range(1, start, start + 3), "40");
        client.diagnostics(uri, 3, json!([]));
        client.shutdown();
    }
}

#[test]
fn implicit_clause_warning_survives_repair_of_an_independent_error() {
    let uri = "file:///fixture/C.scala";
    let text = concat!(
        "class C {\n",
        "  def f(implicit x: Int): Int = x\n",
        "  def only(n: Int): Int = n\n",
        "  def use(): Int = /* 😀 */ f(1)\n",
        "  def bad(): Int = /* 😀 */ only(false)\n",
        "}",
    );
    for (encoding, one, boolean) in [("utf-8", 32, 35), ("utf-16", 30, 33)] {
        let mut client = Client::start(encoding, vec![]);
        let warning = json!({
            "range": range(3, one, one + 1), "severity": 2, "source": "dotty",
            "code": "typer.implicit_clause_without_using",
            "message": "Implicit parameters should be provided with a `using` clause."
        });
        let error = json!({
            "range": range(4, boolean, boolean + 5), "severity": 1, "source": "dotty",
            "code":"typer.application_argument_type_mismatch", "message": "Expected Int, found Boolean"
        });
        let lookup = json!({"textDocument":{"uri":uri}, "position":{"line":3,"character":one-2}});
        let target = json!([{"originSelectionRange":range(3,one-2,one-1),"targetUri":uri,
            "targetSelectionRange":range(1,6,7), "targetRange":range(1,2,"  def f(implicit x: Int): Int = x".len())}]);
        client.open(uri, text);
        client.diagnostics(uri, 1, json!([warning, error]));
        assert_eq!(
            client.request("textDocument/definition", lookup.clone()),
            target
        );
        client.edit(uri, 2, range(4, boolean, boolean + 5), "42");
        client.diagnostics(uri, 2, json!([warning]));
        assert_eq!(client.request("textDocument/definition", lookup), target);
        client.edit(uri, 3, range(3, one, one), "using ");
        client.diagnostics(uri, 3, json!([]));
        client.shutdown();
    }
}

#[test]
fn identical_call_errors_clear_independently_across_edits_and_encodings() {
    let uri = "file:///fixture/Consumer.scala";
    let text = concat!(
        "class Consumer {\n",
        "  def take(n: Int): Int = n\n",
        "  val first = /* 😀 */ take(false)\n",
        "  val other = /* 😀 */ take(false)\n",
        "}",
    );
    for (encoding, column) in [("utf-8", 30), ("utf-16", 28)] {
        let diagnostic = |line| {
            json!({
                "range": range(line, column, column + 5), "severity": 1, "source": "dotty",
                "code":"typer.application_argument_type_mismatch", "message":"Expected Int, found Boolean"
            })
        };
        let mut client = Client::start(encoding, vec![]);
        client.open(uri, text);
        client.diagnostics(uri, 1, json!([diagnostic(2), diagnostic(3)]));
        client.edit(uri, 2, range(2, column, column + 5), "1");
        client.diagnostics(uri, 2, json!([diagnostic(3)]));
        client.edit(uri, 3, range(3, column, column + 5), "2");
        client.diagnostics(uri, 3, json!([]));
        client.shutdown();
    }
}

#[test]
fn repairing_a_cross_file_header_cycle_clears_only_its_participants() {
    let leaf = "file:///fixture/Leaf.scala";
    let prefix = "/* 😀 */ ";
    let a = "class A(x: Int)";
    let b = "class B(x: Int)";
    let parent_a = " extends A(x)";
    let parent_b = " extends B(x)";
    let sibling = "class Other { def value(): Int = 7; def bad(): Int = value(1) }";
    for (encoding, start) in [
        ("utf-8", prefix.len()),
        ("utf-16", prefix.encode_utf16().count()),
    ] {
        let mut client = Client::start(encoding, vec![]);
        let bad = sibling.find("value(1)").unwrap();
        let independent = json!({
            "range": range(1, bad, bad + "value(1)".len()), "severity":1, "source":"dotty",
            "code":"typer.application_arity_mismatch", "message":"Expected 0 arguments, got 1"
        });
        let cycle = |end| {
            json!({
                "range":range(0, start, end), "severity":1,"source":"dotty",
                "code":"signature.header_cycle","message":"Cyclic class header dependency"
            })
        };
        client.open(A, &format!("{prefix}{a}"));
        client.diagnostics(A, 1, json!([]));
        client.open(B, &format!("{prefix}{b}{parent_a}"));
        client.diagnostics(A, 1, json!([]));
        client.diagnostics(B, 1, json!([]));

        client.open(leaf, &format!("class Leaf(x: Int) extends A(x)\n{sibling}"));
        client.diagnostics(A, 1, json!([]));
        client.diagnostics(B, 1, json!([]));
        client.diagnostics(leaf, 1, json!([independent]));

        let end = start + a.len();
        client.edit(A, 2, range(0, end, end), parent_b);
        client.diagnostics(A, 2, json!([cycle(end + parent_b.len())]));
        client.diagnostics(B, 1, json!([cycle(start + b.len() + parent_a.len())]));
        client.diagnostics(leaf, 1, json!([independent]));
        let end = start + b.len();
        client.edit(B, 2, range(0, end, end + parent_a.len()), "");
        client.diagnostics(A, 2, json!([]));
        client.diagnostics(B, 2, json!([]));
        client.diagnostics(leaf, 1, json!([independent]));
        client.shutdown();
    }
}

#[test]
fn repairing_a_provider_restores_the_dependent_call_and_keeps_a_sibling_error() {
    let seeds = "file:///fixture/Seeds.scala";
    let prefix = "  /* 😀 */ val seed = ";
    let middle_method = "  def take() = seeds.seed";
    let caller = "  def read(m: Middle) = m.take()";
    let bad = "  def bad(): Int = only(1)";
    let only = "  def only(): Int = 7";
    let good = "  def good(): Int = pick(true)";
    let overload = "  def pick(b: Boolean): Int = 7";
    let origin = good.find("pick").unwrap();
    let sibling_lookup =
        json!({"textDocument":{"uri":B}, "position":{"line":4,"character":origin+1}});
    let sibling_link = json!([{
        "originSelectionRange":range(4,origin,origin+4), "targetUri":B,
        "targetSelectionRange":range(6,6,10), "targetRange":range(6,2,overload.len())
    }]);
    let call = caller.find("take").unwrap();
    let lookup = json!({"textDocument":{"uri":B}, "position":{"line":1,"character":call+1}});
    let link = json!([{
        "originSelectionRange":range(1,call,call+4), "targetUri":A,
        "targetSelectionRange":range(1,6,10), "targetRange":range(1,2,middle_method.len())
    }]);
    let arity = |line, start, end| {
        json!([{"range":range(line,start,end), "severity":1,
        "source":"dotty","code":"typer.application_arity_mismatch","message":"Expected 0 arguments, got 1"}])
    };
    let independent = arity(3, bad.find("only").unwrap(), bad.len());
    for (encoding, start) in [
        ("utf-8", prefix.len()),
        ("utf-16", prefix.encode_utf16().count()),
    ] {
        let mut client = Client::start(encoding, vec![A, seeds]);
        client.open(
            seeds,
            &format!("class Seeds {{\n  def value(): Int = 40\n{prefix}value()\n}}"),
        );
        client.open(
            A,
            &format!("class Middle(seeds: Seeds) {{\n{middle_method}\n}}"),
        );
        client.open(
            B,
            &format!("class Consumer {{\n{caller}\n{only}\n{bad}\n{good}\n  def pick(n: Int): Int = n\n{overload}\n}}"),
        );
        client.diagnostics(B, 1, independent.clone());
        assert_eq!(
            client.request("textDocument/definition", sibling_lookup.clone()),
            sibling_link
        );
        assert_eq!(
            client.request("textDocument/definition", lookup.clone()),
            link
        );

        let argument = start + "value(".len();
        client.edit(seeds, 2, range(2, argument, argument), "1");
        client.diagnostics(B, 1, independent.clone());
        client.diagnostics(seeds, 2, arity(2, start, start + "value(1)".len()));
        assert_eq!(
            client.request("textDocument/definition", sibling_lookup.clone()),
            sibling_link
        );
        assert_eq!(
            client.request("textDocument/definition", lookup.clone()),
            Value::Null
        );

        client.edit(seeds, 3, range(2, argument, argument + 1), "");
        client.diagnostics(B, 1, independent.clone());
        client.diagnostics(seeds, 3, json!([]));
        assert_eq!(
            client.request("textDocument/definition", sibling_lookup.clone()),
            sibling_link
        );
        assert_eq!(
            client.request("textDocument/definition", lookup.clone()),
            link
        );
        client.shutdown();
    }
}

fn token_column(encoding: &str, line: &str, token: &str) -> usize {
    let prefix = &line[..line.find(token).unwrap()];
    if encoding == "utf-8" {
        prefix.len()
    } else {
        prefix.encode_utf16().count()
    }
}

#[test]
fn public_fields_navigate_from_reads_writes_and_declarations_in_both_encodings() {
    let providers = "/* 😀 */ class A(var count: Int)\nclass B { var count: Int = 2 }";
    let read = "  def read(a: A): Int = /* 😀 */ a.count";
    let write = "  def set(b: A): Unit = /* 😀 */ b.count = 1";
    for encoding in ["utf-8", "utf-16"] {
        let column = |line: &str, token: &str| token_column(encoding, line, token);
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, providers);
        client.open(B, &format!("class Use {{\n{read}\n{write}\n}}"));
        client.diagnostics(B, 1, json!([]));
        let target = column(providers, "count");
        let name = range(0, target, target + 5);
        let decl = json!({"uri":A,"range":name});
        let r = column(read, "count");
        let w = column(write, "count");
        let read_site = json!({"uri":B,"range":range(1,r,r+5)});
        let write_site = json!({"uri":B,"range":range(2,w,w+5)});
        for (uri, line, col) in [(A, 0, target), (B, 1, r), (B, 2, w)] {
            assert_eq!(
                client.references(uri, line, col, true),
                json!([decl, read_site, write_site])
            );
            assert_eq!(
                client.references(uri, line, col, false),
                json!([read_site, write_site])
            );
        }
        for (line, col) in [(1, r), (2, w)] {
            assert_eq!(
                client.request(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":B},"position":{"line":line,"character":col+1}})
                ),
                json!([{"originSelectionRange":range(line,col,col+5),"targetUri":A,"targetSelectionRange":name,"targetRange":range(0,target-4,target+10)}])
            );
        }
        let other = column(providers.lines().nth(1).unwrap(), "count");
        assert_eq!(client.references(A, 1, other, false), json!([]));
        let receiver = column(write, "A");
        client.edit(B, 2, range(2, receiver, receiver + 1), "B");
        client.diagnostics(B, 2, json!([]));
        assert_eq!(client.references(A, 0, target, false), json!([read_site]));
        assert_eq!(client.references(A, 1, other, false), json!([write_site]));
        let write_position = json!({"textDocument":{"uri":B},"position":{"line":2,"character":w}});
        assert_eq!(
            client.request("textDocument/definition", write_position.clone()),
            json!([{"originSelectionRange":write_site["range"],"targetUri":A,"targetSelectionRange":range(1,other,other+5),"targetRange":range(1,other-4,other+"count: Int = 2".len())}])
        );

        let rhs = column(write, "1");
        client.edit(B, 3, range(2, rhs, rhs + 1), "false");
        client.diagnostics(B,3,json!([{"range":range(2,rhs,rhs+5),"severity":1,"source":"dotty",
            "code":"typer.expected_expression_type_mismatch","message":"Expression does not conform to the expected type"}]));
        assert_eq!(client.references(A, 1, other, false), json!([]));
        assert_eq!(
            client.request("textDocument/definition", write_position),
            Value::Null
        );
        assert_eq!(client.references(B, 1, r, true), json!([decl, read_site]));

        client.shutdown();
    }
}

#[test]
fn own_and_external_field_navigation_share_declarations_in_both_encodings() {
    let lines = [
        "class A {",
        "  var count: Int = 0",
        "  private var hidden: Int = 1",
        "  def read(other: Int): Int = /* 😀 */ count",
        "  def keep(): Int = this.count",
        "  def copy(): Unit = /* 😀 */ this.hidden = hidden",
        "}",
    ];
    let external = "class B { def get(a: A): Int = a.count }";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A, B]);
        client.open(A, &lines.join("\n"));
        client.open(B, external);
        let col = |line: usize, token: &str| token_column(encoding, lines[line], token);
        let loc = |uri: &str, line: usize, col: usize, len: usize| json!({"uri":uri,"range":range(line,col,col+len)});
        let count = loc(A, 1, col(1, "count"), 5);
        let read = loc(A, 3, col(3, "count"), 5);
        let keep = loc(A, 4, col(4, "count"), 5);
        let get = loc(B, 0, token_column(encoding, external, "count"), 5);
        assert_eq!(
            client.references(A, 3, col(3, "count"), true),
            json!([count, read, keep, get])
        );
        assert_eq!(
            client.references(B, 0, token_column(encoding, external, "count"), false),
            json!([read, keep, get])
        );
        let private = loc(A, 2, col(2, "hidden"), 6);
        let write = col(5, "hidden");
        let rhs = col(5, "= hidden") + 2;
        assert_eq!(
            client.references(A, 5, rhs, true),
            json!([private, loc(A, 5, write, 6), loc(A, 5, rhs, 6)])
        );
        let lookup = |line, character| json!({"textDocument":{"uri":A}, "position":{"line":line,"character":character}});
        for (line, name, len, decl_line, decl_name) in [
            (3, col(3, "count"), 5, 1, col(1, "count")),
            (5, write, 6, 2, col(2, "hidden")),
        ] {
            assert_eq!(
                client.request("textDocument/definition", lookup(line, name)),
                json!([{
                    "originSelectionRange":range(line,name,name+len), "targetUri":A,
                    "targetSelectionRange":range(decl_line,decl_name,decl_name+len),
                    "targetRange":range(decl_line,2,lines[decl_line].len())
                }])
            );
        }
        let parameter = col(3, "other");
        let use_site = col(3, "count");
        let formal = loc(A, 3, parameter, 5);
        assert_eq!(client.references(A, 3, parameter, true), json!([formal]));
        // Equal-length replacement keeps the written usage at exactly the same position.
        for (version, replacement) in [(2, "count"), (3, "other")] {
            client.edit(A, version, range(3, parameter, parameter + 5), replacement);
            client.diagnostics(A, version, json!([]));
            let parameter_sites = if version == 2 {
                json!([formal, read])
            } else {
                json!([formal])
            };
            assert_eq!(client.references(A, 3, parameter, true), parameter_sites);
            let (expected_sites, expected_definition) = if version == 2 {
                (
                    json!([keep, get]),
                    json!([{
                        "originSelectionRange":read["range"], "targetUri":A,
                        "targetSelectionRange":range(3, parameter, parameter+5), "targetRange":range(3,parameter,parameter+"count: Int".len())
                    }]),
                )
            } else {
                (
                    json!([read, keep, get]),
                    json!([{
                        "originSelectionRange":read["range"], "targetUri":A,
                        "targetSelectionRange":count["range"], "targetRange":range(1,2,lines[1].len())
                    }]),
                )
            };
            assert_eq!(
                client.references(A, 4, col(4, "count"), false),
                expected_sites
            );
            assert_eq!(
                client.request("textDocument/definition", lookup(3, use_site)),
                expected_definition
            );
        }
        client.shutdown();
    }
}

#[test]
fn overloaded_method_parameters_navigate_by_symbol_in_both_encodings() {
    let lines = [
        "class A {",
        "  def pick(/* 😀 */ x: Int): Int = x + x",
        "  def pick(/* 😀 */ x: Boolean): Boolean = x",
        "}",
    ];
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &lines.join("\n"));
        for (line, formal) in [(1, "x: Int"), (2, "x: Boolean")] {
            let col = |token| token_column(encoding, lines[line], token);
            let decl = col(formal);
            let read = col("= x") + 2;
            let location = |x| json!({"uri":A,"range":range(line,x,x+1)});
            let mut uses = vec![location(read)];
            if line == 1 {
                uses.push(location(col("+ x") + 2));
            }
            let mut all = vec![location(decl)];
            all.extend_from_slice(&uses);
            for pos in [decl, read] {
                assert_eq!(client.references(A, line, pos, true), json!(all));
                assert_eq!(client.references(A, line, pos, false), json!(uses));
            }
            assert_eq!(
                client.request(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":A},"position":{"line":line,"character":read}})
                ),
                json!([{"originSelectionRange":range(line,read,read+1),"targetUri":A,
                "targetRange":range(line,decl,decl+formal.len()),"targetSelectionRange":range(line,decl,decl+1)}])
            );
        }
        client.shutdown();
    }
}

#[test]
fn nested_locals_keep_distinct_references_when_a_sibling_body_fails() {
    let lines = [
        "class A {",
        "  def read(): Int = { /* 😀 */ val x = 1;",
        "    val before = x;",
        "    { /* 😀 */ val x = 2; x };",
        "    before + x }",
        "  def only(i: Int): Int = i",
        "  def bad(): Int = { val lost = 4; only(lost) }",
        "}",
    ];
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &lines.join("\n"));
        let col = |line: usize, token: &str| token_column(encoding, lines[line], token);
        let location = |line, col| json!({"uri":A,"range":range(line,col,col+1)});
        let lost = col(6, "lost =");
        let arg = col(6, "only(") + 5;
        assert_eq!(
            client.references(A, 6, lost, false),
            json!([{"uri":A,"range":range(6,arg,arg+4)}])
        );
        for version in [1, 2] {
            if version == 2 {
                client.edit(A, 2, range(6, arg, arg + 4), "false");
                client.diagnostics(A,2,json!([{"range":range(6,arg,arg+5),"source":"dotty","severity":1,
                    "code":"typer.application_argument_type_mismatch","message":"Expected Int, found Boolean"}]));
                assert_eq!(client.references(A, 6, lost, true), Value::Null);
            }
            for (decl_line, read_sites) in [
                (1, vec![(2, col(2, "x;")), (4, col(4, "x }"))]),
                (3, vec![(3, col(3, "x }"))]),
            ] {
                let decl = col(decl_line, "x =");
                let reads: Vec<_> = read_sites.iter().map(|&(l, c)| location(l, c)).collect();
                let mut all = vec![location(decl_line, decl)];
                all.extend_from_slice(&reads);
                for (line, c) in read_sites {
                    assert_eq!(client.references(A, line, c, false), json!(reads));
                    assert_eq!(client.references(A, line, c, true), json!(all));
                    assert_eq!(
                        client.request(
                            "textDocument/definition",
                            json!({"textDocument":{"uri":A},"position":{"line":line,"character":c}})
                        ),
                        json!([{"originSelectionRange":range(line,c,c+1),"targetUri":A,
                        "targetRange":range(decl_line,decl-4,decl+5),"targetSelectionRange":range(decl_line,decl,decl+1)}])
                    );
                }
            }
        }
        client.shutdown();
    }
}

#[test]
fn renaming_a_local_redirects_the_same_read_from_its_parameter_and_back() {
    let line = "class A { def read(count: Int): Int = { /* 😀 */ val other = 1; count } }";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, line);
        let col = |t| token_column(encoding, line, t);
        let param = col("count:");
        let local = col("other");
        let read = col("count }");
        let loc = |c| json!({"uri":A,"range":range(0,c,c+5)});
        for (version, replacement) in [(1, "other"), (2, "count"), (3, "other")] {
            if version != 1 {
                client.edit(A, version, range(0, local, local + 5), replacement);
                client.diagnostics(A, version, json!([]));
            }
            let (name, declaration, param_uses, local_uses) = if replacement == "count" {
                (
                    local,
                    range(0, local - 4, local + 9),
                    json!([]),
                    json!([loc(read)]),
                )
            } else {
                (
                    param,
                    range(0, param, param + "count: Int".len()),
                    json!([loc(read)]),
                    json!([]),
                )
            };
            assert_eq!(client.references(A, 0, param, false), param_uses);
            assert_eq!(client.references(A, 0, local, false), local_uses);
            assert_eq!(
                client.references(A, 0, read, true),
                json!([loc(name), loc(read)])
            );
            assert_eq!(
                client.request(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":A},"position":{"line":0,"character":read}})
                ),
                json!([{"originSelectionRange":loc(read)["range"],"targetUri":A,"targetRange":declaration,
                "targetSelectionRange":loc(name)["range"]}])
            );
        }
        client.shutdown();
    }
}

#[test]
fn constructor_parameters_navigate_across_headers_and_bodies_in_both_encodings() {
    let base = "class Base(/* 😀 */ left: Int) { val initial: Int = left }";
    let child = "class Derived(/* 😀 */ left: Int, next: Int) extends Base(/* 😀 */ left) { val own: Int = left }";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A, B]);
        client.open(A, base);
        client.open(B, child);
        let col = |line, token| token_column(encoding, line, token);
        let base_decl = col(base, "left:");
        let base_read = col(base, "left }");
        let child_decl = col(child, "left:");
        let parent_arg = col(child, "left)");
        let child_read = col(child, "left }");
        let loc = |uri: &str, c| json!({"uri":uri,"range":range(0,c,c+4)});
        for (uri, decl, uses) in [
            (A, base_decl, vec![base_read]),
            (B, child_decl, vec![parent_arg, child_read]),
        ] {
            let reads: Vec<_> = uses.iter().map(|&c| loc(uri, c)).collect();
            let mut all = vec![loc(uri, decl)];
            all.extend_from_slice(&reads);
            assert_eq!(client.references(uri, 0, decl, true), json!(all));
            for pos in uses {
                assert_eq!(client.references(uri, 0, pos, false), json!(reads));
                assert_eq!(
                    client.request(
                        "textDocument/definition",
                        json!({"textDocument":{"uri":uri},"position":{"line":0,"character":pos}})
                    ),
                    json!([{"originSelectionRange":range(0,pos,pos+4),"targetUri":uri,
                    "targetRange":range(0,decl,decl+"left: Int".len()),"targetSelectionRange":range(0,decl,decl+4)}])
                );
            }
        }
        let next_decl = col(child, "next:");
        assert_eq!(client.references(B, 0, next_decl, false), json!([]));
        for (version, replacement) in [(2, "next"), (3, "left")] {
            client.edit(
                B,
                version,
                range(0, parent_arg, parent_arg + 4),
                replacement,
            );
            client.diagnostics(B, version, json!([]));
            let (target, left_reads, next_reads) = if replacement == "next" {
                (
                    next_decl,
                    json!([loc(B, child_read)]),
                    json!([loc(B, parent_arg)]),
                )
            } else {
                (
                    child_decl,
                    json!([loc(B, parent_arg), loc(B, child_read)]),
                    json!([]),
                )
            };
            assert_eq!(client.references(B, 0, child_decl, false), left_reads);
            assert_eq!(client.references(B, 0, next_decl, false), next_reads);
            assert_eq!(
                client.references(A, 0, base_decl, false),
                json!([loc(A, base_read)])
            );
            assert_eq!(
                client.request(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":B},"position":{"line":0,"character":parent_arg}})
                ),
                json!([{"originSelectionRange":range(0,parent_arg,parent_arg+4),"targetUri":B,
                "targetRange":range(0,target,target+"next: Int".len()),"targetSelectionRange":range(0,target,target+4)}])
            );
        }
        client.shutdown();
    }
}

#[test]
fn superclass_navigation_separates_the_parent_type_from_its_argument() {
    let base = "/* 😀 */ class Base(n: Int)";
    let peer = "/* 😀 */ class Peer(n: Int)";
    let kept = "class Kept(n: Int) extends Base(n)";
    let child = "class Child(/* 😀 */ start: Int) extends /* 😀 */ Base(start)";
    for encoding in ["utf-8", "utf-16"] {
        let col = |line, token| token_column(encoding, line, token);
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &[base, peer, kept].join("\n"));
        client.open(B, child);
        client.diagnostics(B, 1, json!([]));
        let use_pos = col(child, "Base");
        let arg_pos = col(child, "start)");
        let arg_decl = col(child, "start:");
        let loc = |uri: &str, line, c| json!({"uri":uri,"range":range(line,c,c+4)});
        let parent_use = loc(B, 0, use_pos);
        let kept_use = loc(A, 2, col(kept, "Base"));
        let query = |pos| json!({"textDocument":{"uri":B},"position":{"line":0,"character":pos}});
        for (version, name, line, provider) in [
            (1, "Base", 0, base),
            (2, "Peer", 1, peer),
            (3, "Base", 0, base),
        ] {
            if version > 1 {
                client.edit(B, version, range(0, use_pos, use_pos + 4), name);
                client.diagnostics(B, version, json!([]));
            }
            let decl = col(provider, name);
            let (base_refs, peer_refs, all) = if name == "Base" {
                (
                    json!([kept_use, parent_use]),
                    json!([]),
                    json!([loc(A, 0, decl), kept_use, parent_use]),
                )
            } else {
                (
                    json!([kept_use]),
                    json!([parent_use]),
                    json!([loc(A, 1, decl), parent_use]),
                )
            };
            assert_eq!(client.references(A, 0, col(base, "Base"), false), base_refs);
            assert_eq!(client.references(A, 1, col(peer, "Peer"), false), peer_refs);
            assert_eq!(client.references(B, 0, use_pos, true), all);
            assert_eq!(
                client.request("textDocument/definition", query(use_pos)),
                json!([{
                    "originSelectionRange":parent_use["range"],"targetUri":A,
                    "targetRange":range(line,col(provider,"class"),decl+"Base(n: Int)".len()),
                    "targetSelectionRange":range(line,decl,decl+4)
                }])
            );
            assert_eq!(
                client.request("textDocument/definition", query(arg_pos)),
                json!([{
                    "originSelectionRange":range(0,arg_pos,arg_pos+5),"targetUri":B,
                    "targetRange":range(0,arg_decl,arg_decl+"start: Int".len()),
                    "targetSelectionRange":range(0,arg_decl,arg_decl+5)
                }])
            );
        }
        client.shutdown();
    }
}

#[test]
fn construction_class_argument_and_method_have_independent_definition_links() {
    let base = "/* 😀 */ class Base(n: Int) { def read(): Int = n }";
    let peer = "/* 😀 */ class Peer(n: Int) { def read(): Int = n }";
    let kept = "class Kept(n: Int) extends Base(n)";
    let factory =
        "class Factory { def build(/* 😀 */ start: Int): Int = /* 😀 */ new Base(start).read() }";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &[base, peer, kept].join("\n"));
        client.open(B, factory);
        client.diagnostics(B, 1, json!([]));
        let col = |line, t| token_column(encoding, line, t);
        let use_pos = col(factory, "Base");
        let arg = col(factory, "start)");
        let read = col(factory, "read()");
        let arg_decl = col(factory, "start:");
        let loc = |uri: &str, line, c| json!({"uri":uri,"range":range(line,c,c+4)});
        let parent = loc(A, 2, col(kept, "Base"));
        let allocation = loc(B, 0, use_pos);
        for (version, name, line, provider) in [
            (1, "Base", 0, base),
            (2, "Peer", 1, peer),
            (3, "Base", 0, base),
        ] {
            if version > 1 {
                client.edit(B, version, range(0, use_pos, use_pos + 4), name);
                client.diagnostics(B, version, json!([]));
            }
            let decl = col(provider, name);
            let (base_refs, peer_refs, all) = if name == "Base" {
                (
                    json!([parent, allocation]),
                    json!([]),
                    json!([loc(A, 0, decl), parent, allocation]),
                )
            } else {
                (
                    json!([parent]),
                    json!([allocation]),
                    json!([loc(A, 1, decl), allocation]),
                )
            };
            assert_eq!(client.references(A, 0, col(base, "Base"), false), base_refs);
            assert_eq!(client.references(A, 1, col(peer, "Peer"), false), peer_refs);
            assert_eq!(client.references(B, 0, use_pos, true), all);
            let method_decl = col(provider, "def read");
            for (pos, len, uri, line, declaration, name) in [
                (
                    use_pos,
                    4,
                    A,
                    line,
                    range(line, col(provider, "class"), col(provider, "}") + 1),
                    decl,
                ),
                (
                    arg,
                    5,
                    B,
                    0,
                    range(0, arg_decl, arg_decl + "start: Int".len()),
                    arg_decl,
                ),
                (
                    read,
                    4,
                    A,
                    line,
                    range(line, method_decl, method_decl + "def read(): Int = n".len()),
                    col(provider, "read"),
                ),
            ] {
                assert_eq!(
                    client.request(
                        "textDocument/definition",
                        json!({"textDocument":{"uri":B},"position":{"line":0,"character":pos}})
                    ),
                    json!([{"originSelectionRange":range(0,pos,pos+len),"targetUri":uri,"targetRange":declaration,"targetSelectionRange":range(line,name,name+len)}])
                );
            }
        }
        client.shutdown();
    }
}

#[test]
fn method_type_annotations_link_to_the_class_and_share_its_references() {
    let base = "/* 😀 */ class Base";
    let peer = "/* 😀 */ class Peer";
    let factory = "class Factory { def copy(/* 😀 */ x: Base): Base = new Base() }";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &[base, peer].join("\n"));
        client.open(B, factory);
        client.diagnostics(B, 1, json!([]));
        let col = |text, token| token_column(encoding, text, token);
        let input = col(factory, "Base)");
        let result = col(factory, "Base =");
        let allocation = col(factory, "Base()");
        let loc = |uri: &str, line, column| json!({"uri":uri,"range":range(line,column,column+4)});
        for (version, param_type) in [(1, "Base"), (2, "Peer"), (3, "Base")] {
            if version > 1 {
                client.edit(B, version, range(0, input, input + 4), param_type);
                client.diagnostics(B, version, json!([]));
            }
            let mut base_uses = vec![loc(B, 0, result), loc(B, 0, allocation)];
            let mut peer_uses = Vec::new();
            if param_type == "Base" {
                base_uses.insert(0, loc(B, 0, input));
            } else {
                peer_uses.push(loc(B, 0, input));
            }
            assert_eq!(
                client.references(A, 0, col(base, "Base"), false),
                json!(base_uses)
            );
            assert_eq!(
                client.references(A, 1, col(peer, "Peer"), false),
                json!(peer_uses)
            );
            for pos in [input, result, allocation] {
                let (line, provider, class, uses) = if pos == input && param_type == "Peer" {
                    (1, peer, "Peer", &peer_uses)
                } else {
                    (0, base, "Base", &base_uses)
                };
                let name = col(provider, class);
                let mut all = vec![loc(A, line, name)];
                all.extend_from_slice(uses);
                assert_eq!(client.references(B, 0, pos, false), json!(uses));
                assert_eq!(client.references(B, 0, pos, true), json!(all));
                assert_eq!(
                    client.request(
                        "textDocument/definition",
                        json!({"textDocument":{"uri":B},"position":{"line":0,"character":pos}})
                    ),
                    json!([{"originSelectionRange":range(0,pos,pos+4),"targetUri":A,
                        "targetRange":range(line,col(provider,"class"),name+4),"targetSelectionRange":range(line,name,name+4)}])
                );
            }
        }
        client.shutdown();
    }
}
