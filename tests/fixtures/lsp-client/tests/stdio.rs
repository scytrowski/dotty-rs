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
        Self::start_with_options(encoding, initially_clean, Value::Null)
    }

    fn start_with_options(
        encoding: &str,
        initially_clean: Vec<&'static str>,
        options: Value,
    ) -> Self {
        Self::start_configured(encoding, initially_clean, options, false)
    }

    fn start_configured(
        encoding: &str,
        initially_clean: Vec<&'static str>,
        options: Value,
        related: bool,
    ) -> Self {
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
                "processId": null, "rootUri": null, "initializationOptions": options,
                "capabilities": {
                    "general": {"positionEncodings": [encoding]},
                    "textDocument": {
                        "definition": {"linkSupport": true},
                        "signatureHelp": {"signatureInformation":{"activeParameterSupport":true}},
                        "publishDiagnostics": {"versionSupport": true, "relatedInformation": related}
                    }
                }
            }),
        );
        assert_eq!(init["capabilities"]["positionEncoding"], encoding);
        assert_eq!(init["capabilities"]["definitionProvider"], true);
        assert_eq!(init["capabilities"]["hoverProvider"], true);
        assert_eq!(
            init["capabilities"]["signatureHelpProvider"]["triggerCharacters"],
            json!(["(", ","])
        );
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

    fn hover(&mut self, uri: &str, line: usize, character: usize) -> Value {
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri":uri}, "position":{"line":line,"character":character}
            }),
        )
    }

    fn signature_help(&mut self, uri: &str, line: usize, character: usize) -> Value {
        self.request(
            "textDocument/signatureHelp",
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}}),
        )
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

    let signature = |line, start, end, label: &str| {
        json!({
            "range":range(line, start, end), "contents":{"kind":"plaintext", "value":label}
        })
    };
    let integer_hover = signature(1, take_column, take_column + 4, "def take(n: Int): Int");
    let boolean_hover = signature(1, take_column, take_column + 4, "def take(b: Boolean): Int");
    assert_eq!(client.hover(B, 1, take_column + 1), integer_hover);
    assert_eq!(client.hover(B, 1, take_column + 4), Value::Null);
    assert_eq!(
        client.hover(A, 2, 6),
        signature(2, 6, 10, "def take(b: Boolean): Int")
    );
    assert_eq!(client.hover(A, 2, 11), signature(2, 11, 12, "b: Boolean"));
    assert_eq!(
        client.hover(B, 1, take_column - 2),
        signature(1, take_column - 2, take_column - 1, "a: A")
    );
    assert_eq!(client.hover(A, 0, 6), signature(0, 6, 7, "class A"));
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
    assert_eq!(client.hover(B, 1, take_column), boolean_hover);
    assert_eq!(client.references(A, 1, 7, false), json!([stable]));
    assert_eq!(
        client.references(B, 1, take_column + 1, true),
        json!([boolean, caller])
    );

    let only_hover = signature(3, 19, 23, "def only(n: Int): Int");
    assert_eq!(client.hover(B, 3, 20), only_hover);
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
    assert_eq!(client.hover(B, 1, take_column), boolean_hover);
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
    assert_eq!(client.hover(B, 3, 20), Value::Null);
    client.edit(B, 4, range(3, 24, 29), "1");
    client.diagnostics(B, 4, json!([]));
    assert_eq!(client.hover(B, 3, 20), only_hover);
    client.definition(take_column, 2, BOOL_METHOD);
    assert_eq!(client.hover(B, 1, take_column), boolean_hover);
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

#[test]
fn constructor_type_annotations_link_to_the_class_and_share_its_references() {
    let model = ["/* 😀 */ class Base", "/* 😀 */ class Peer"];
    let holder = "class Holder(/* 😀 */ seed: Base, val first: Base, var next: Peer)";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &model.join("\n"));
        client.open(B, holder);
        client.diagnostics(B, 1, json!([]));
        let col = |text, token| token_column(encoding, text, token);
        let positions = [
            col(holder, "Base, val"),
            col(holder, "Base, var"),
            col(holder, "Peer)"),
        ];
        // The edit moves only the middle token between the two ordered reference lists.
        for (version, first_type, split) in [(1, "Base", 2), (2, "Peer", 1), (3, "Base", 2)] {
            if version > 1 {
                client.edit(
                    B,
                    version,
                    range(0, positions[1], positions[1] + 4),
                    first_type,
                );
                client.diagnostics(B, version, json!([]));
            }
            for (line, class, offsets) in [
                (0, "Base", &positions[..split]),
                (1, "Peer", &positions[split..]),
            ] {
                let name = col(model[line], class);
                let name_range = range(line, name, name + class.len());
                let target_range = range(line, col(model[line], "class"), name + class.len());
                let uses: Vec<_> = offsets
                    .iter()
                    .map(|&pos| json!({"uri":B,"range":range(0,pos,pos+class.len())}))
                    .collect();
                assert_eq!(client.references(A, line, name, false), json!(uses));
                let mut all = vec![json!({"uri":A,"range":name_range})];
                all.extend_from_slice(&uses);
                for &pos in offsets {
                    assert_eq!(client.references(B, 0, pos, true), json!(all));
                    assert_eq!(
                        client.request(
                            "textDocument/definition",
                            json!({"textDocument":{"uri":B},"position":{"line":0,"character":pos}})
                        ),
                        json!([{"targetUri":A,"targetRange":target_range,"targetSelectionRange":name_range,"originSelectionRange":range(0,pos,pos+class.len())}])
                    );
                }
            }
        }
        client.shutdown();
    }
}

#[path = "../../external-dependency/fixture.rs"]
mod external_fixture;

#[test]
fn removing_a_compiled_dependency_reports_its_exact_type_token() {
    use dotty::core::DocumentText;
    use dotty::{SemanticIssue, SemanticPhase, typer::TyperError};
    use dotty_lsp::analysis::{Document, Problem, analyze_with_classpath};
    let fixture = external_fixture::ExternalDependency::compile();
    let path = fixture.classpath();
    let line = "class Holder(/* 😀 */ x: External) { def value(): External = x }";
    let text = format!("package demo\n{line}");
    let docs = [(
        B.to_owned(),
        Document {
            version: 1,
            text: std::sync::Arc::new(DocumentText::new(text.clone()).unwrap()),
        },
    )]
    .into();
    assert!(
        analyze_with_classpath(&docs, Some(&path)).files[B]
            .problems
            .is_empty()
    );
    let clients: Vec<_> = ["utf-8", "utf-16"]
        .into_iter()
        .map(|encoding| {
            let mut client = Client::start_with_options(
                encoding,
                vec![],
                json!({"classDirectory":fixture.directory}),
            );
            client.open(B, &text);
            client.diagnostics(B, 1, json!([]));
            let declaration = token_column(encoding, line, "x:");
            let usage = token_column(encoding, line, "= x") + 2;
            let definition = client.request(
                "textDocument/definition",
                json!({"textDocument":{"uri":B},"position":{"line":1,"character":usage}}),
            );
            assert_eq!(
                definition,
                json!([{
                    "originSelectionRange": range(1, usage, usage + 1),
                    "targetUri": B,
                    "targetRange": range(1, declaration, declaration + "x: External".len()),
                    "targetSelectionRange": range(1, declaration, declaration + 1)
                }])
            );
            (encoding, client)
        })
        .collect();
    fixture.remove_external();
    let analysis = analyze_with_classpath(&docs, Some(&path));
    let [Problem::Semantic(issue)] = analysis.files[B].problems.as_slice() else {
        panic!("one semantic diagnostic")
    };
    assert_eq!(issue.phase, SemanticPhase::Constructor);
    let SemanticIssue::Typing(report) = &issue.issue else {
        panic!("typed error")
    };
    assert!(
        matches!(report.error(),TyperError::TypeNameNotFound {source,..} if Some(*source)==analysis.source(B))
    );
    let location = report.location().unwrap();
    assert_eq!(Some(location.source()), analysis.source(B));
    let range_bytes = location.span().range();
    assert_eq!(
        &text[range_bytes.start() as usize..range_bytes.end() as usize],
        "External"
    );
    for (encoding, mut client) in clients {
        // Reopen the same text to trigger fresh analysis in the running server.
        client.notify("textDocument/didClose", json!({"textDocument":{"uri":B}}));
        let Message::Notification(closed) = client.receive() else {
            panic!("close diagnostic")
        };
        assert_eq!(closed.method, "textDocument/publishDiagnostics");
        assert_eq!(closed.params, json!({"uri":B,"diagnostics":[]}));
        client.open(B, &text);
        let col = token_column(encoding, line, "External");
        client.diagnostics(B, 1, json!([{
            "range": range(1, col, col + "External".len()), "severity": 1,
            "source": "dotty", "code": "typer.type_name_not_found", "message": "Unknown type name"
        }]));
        client.shutdown();
    }
}

#[test]
fn signature_help_tracks_the_active_formal_and_selected_overload_across_edits() {
    let methods = "class Ops { def pick(n: Int, b: Boolean): Int = n; def pick(b: Boolean, n: Int): Int = n; def empty(): Int = 0 }";
    let line = "  def run(o: Ops): Int = /* 😀 */ o.pick(42, false)";
    let nested = "  def nested(o: Ops): Int = o.pick(o.empty(), false)";
    let expected = |first: &str, second: &str, active| {
        json!({
            "signatures":[{"label":format!("def pick({first}, {second}): Int"),
                "parameters":[{"label":first},{"label":second}]}],
            "activeSignature":0, "activeParameter":active,
        })
    };
    for enc in ["utf-8", "utf-16"] {
        let mut client = Client::start(enc, vec![A]);
        client.open(A, methods);
        client.open(B, &format!("class Use {{\r\n{line}\r\n{nested}\r\n  def only(n: Int): Int = n\r\n  def bad(): Int = only(1)\r\n}}"));
        client.diagnostics(B, 1, json!([]));
        let query = |client: &mut Client, line: usize, character: usize| {
            client.request(
                "textDocument/signatureHelp",
                json!({"textDocument":{"uri":B},"position":{"line":line,"character":character}}),
            )
        };
        let start = token_column(enc, line, "42");
        for (delta, active) in [(0, 0), (2, 0), (3, 1), (4, 1)] {
            assert_eq!(
                query(&mut client, 1, start + delta),
                expected("n: Int", "b: Boolean", active)
            );
        }
        assert_eq!(query(&mut client, 1, start - 1), Value::Null);
        assert_eq!(
            query(&mut client, 2, token_column(enc, nested, "empty()") + 6),
            json!({
                "signatures":[{"label":"def empty(): Int","parameters":[]}], "activeSignature":0
            })
        );
        assert_eq!(
            query(&mut client, 2, token_column(enc, nested, ", false") + 1),
            expected("n: Int", "b: Boolean", 1)
        );
        client.edit(B, 2, range(1, start, start + "42, false".len()), "true, 7");
        client.diagnostics(B, 2, json!([]));
        assert_eq!(
            query(&mut client, 1, start + 5),
            expected("b: Boolean", "n: Int", 1)
        );
        client.edit(B, 3, range(4, 24, 25), "false");
        client.diagnostics(B, 3, json!([{
            "range":range(4,24,29), "severity":1, "source":"dotty",
            "code":"typer.application_argument_type_mismatch", "message":"Expected Int, found Boolean"
        }]));
        assert_eq!(query(&mut client, 4, 24), Value::Null);
        assert_eq!(
            query(&mut client, 1, start),
            expected("b: Boolean", "n: Int", 0)
        );
        client.shutdown();
    }
}

const PICK_OVERLOADS: &str = "class Ops { def pick(a: Int, b: Boolean, n: Int, tail: Boolean): Int = n; def pick(a: Int, b: Boolean, flag: Boolean, tail: Int): Int = tail }";

fn pick_signature(third: &str, tail: &str) -> Value {
    json!({
        "label":format!("def pick(a: Int, b: Boolean, {third}, {tail}): Int"),
        "parameters":[{"label":"a: Int"},{"label":"b: Boolean"},{"label":third},{"label":tail}]
    })
}

fn expect_missing_argument(client: &mut Client, version: i32) {
    let Message::Notification(n) = client.receive() else {
        panic!("syntax diagnostics")
    };
    assert_eq!(n.method, "textDocument/publishDiagnostics");
    assert_eq!(n.params["uri"], B);
    assert_eq!(n.params["version"], version);
    assert!(
        n.params["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "parser.expression.missing_argument")
    );
}

#[test]
fn incomplete_signature_help_narrows_at_the_third_argument_and_commits_after_closing() {
    let line = "  def run(o: Ops): Int = /* 😀 */ o.pick(";
    let first = pick_signature("n: Int", "tail: Boolean");
    let second = pick_signature("flag: Boolean", "tail: Int");
    for enc in ["utf-8", "utf-16"] {
        let mut client = Client::start(enc, vec![A]);
        client.open(A, PICK_OVERLOADS);
        client.open(B, &format!("class Use {{\r\n{line}\r\n}}"));
        let start = token_column(enc, line, "pick(") + 5;
        let mut previous = "";
        for (i, (args, slot, options)) in [
            ("", 0, vec![&first, &second]),
            ("1, ", 1, vec![&first, &second]),
            ("1, true, ", 2, vec![&first, &second]),
            ("1, true, 2, ", 3, vec![&first]),
            ("1, true, false, ", 3, vec![&second]),
            ("true, ", 1, vec![]),
            ("1, true, 2, false)", 3, vec![&first]),
        ]
        .into_iter()
        .enumerate()
        {
            if i != 0 {
                client.edit(
                    B,
                    i as i32 + 1,
                    range(1, start, start + previous.len()),
                    args,
                );
            }
            let closed = args.ends_with(')');
            if closed {
                client.diagnostics(B, i as i32 + 1, json!([]));
            } else {
                expect_missing_argument(&mut client, i as i32 + 1);
            }
            let pos = start + args.len() - usize::from(closed);
            let help = client.request(
                "textDocument/signatureHelp",
                json!({"textDocument":{"uri":B},"position":{"line":1,"character":pos}}),
            );
            let expected = if options.is_empty() {
                Value::Null
            } else if closed {
                json!({"signatures":options,"activeParameter":slot,"activeSignature":0})
            } else {
                json!({"signatures":options,"activeParameter":slot})
            };
            assert_eq!(help, expected, "{enc} at {args:?}");
            let hover = client.hover(B, 1, start - 3);
            assert_eq!(hover.is_null(), !closed);
            previous = args;
        }
        client.shutdown();
    }
}

#[test]
fn editing_a_local_initializer_changes_signature_help_at_the_unchanged_call() {
    let local = "    /* 😀 */ val third = 2";
    for enc in ["utf-8", "utf-16"] {
        for (call, completion) in [
            ("    /* 😀 */ ops.pick(1, true, third, ", "false)"),
            (
                "    /* 😀 */ val result = ops.pick(1, true, third, ",
                "false); result",
            ),
        ] {
            let mut client = Client::start(enc, vec![A]);
            client.open(A, PICK_OVERLOADS);
            client.open(
                B,
                &format!(
                    "class Use {{
  def run(actual: Ops): Int = {{
    val ops = actual
{local}
{call}
  }}
}}"
                ),
            );
            let edit_start = token_column(enc, local, "2");
            let cursor = token_column(enc, call, "third, ") + "third, ".len();
            for (version, edit, expected) in [
                (1, None, pick_signature("n: Int", "tail: Boolean")),
                (
                    2,
                    Some(("2", "false")),
                    pick_signature("flag: Boolean", "tail: Int"),
                ),
                (
                    3,
                    Some(("false", "2")),
                    pick_signature("n: Int", "tail: Boolean"),
                ),
            ] {
                if let Some((old, new)) = edit {
                    client.edit(
                        B,
                        version,
                        range(3, edit_start, edit_start + old.len()),
                        new,
                    );
                }
                expect_missing_argument(&mut client, version);
                let help = client.request(
                    "textDocument/signatureHelp",
                    json!({"textDocument":{"uri":B},"position":{"line":4,"character":cursor}}),
                );
                assert_eq!(
                    help,
                    json!({"signatures":[expected],"activeParameter":3}),
                    "{enc}, revision {version}"
                );
            }
            client.edit(B, 4, range(4, cursor, cursor), completion);
            client.diagnostics(B, 4, json!([]));
            let end = cursor + "false".len();
            let committed = client.request(
                "textDocument/signatureHelp",
                json!({"textDocument":{"uri":B},"position":{"line":4,"character":end}}),
            );
            assert_eq!(
                committed,
                json!({
                    "signatures": [pick_signature("n: Int", "tail: Boolean")],
                    "activeParameter": 3,
                    "activeSignature": 0
                }),
                "{enc}: {call}"
            );
            client.shutdown();
        }
    }
}

#[test]
fn shadowing_a_bare_call_removes_signature_help_until_undo() {
    let methods = PICK_OVERLOADS.strip_suffix('}').unwrap();
    let prefix = "    /* 😀 */ val third = 2";
    let shadow = "; val pick = 0";
    let call = "    /* 😀 */ val result = pick(1, true, third, ";
    for enc in ["utf-8", "utf-16"] {
        let mut client = Client::start(enc, vec![]);
        client.open(
            B,
            &format!(
                "{methods}
  def run: Int = {{
{prefix}
{call}
  }}
}}"
            ),
        );
        let edit = token_column(enc, prefix, "2") + 1;
        let cursor = token_column(enc, call, "third, ") + "third, ".len();
        let expected =
            json!({"signatures":[pick_signature("n: Int", "tail: Boolean")],"activeParameter":3});
        expect_missing_argument(&mut client, 1);
        assert_eq!(client.signature_help(B, 3, cursor), expected);
        client.edit(B, 2, range(2, edit, edit), shadow);
        expect_missing_argument(&mut client, 2);
        assert_eq!(client.signature_help(B, 3, cursor), Value::Null);
        client.edit(B, 3, range(2, edit, edit + shadow.len()), "");
        expect_missing_argument(&mut client, 3);
        assert_eq!(client.signature_help(B, 3, cursor), expected);
        client.edit(B, 4, range(3, cursor, cursor), "false); result");
        client.diagnostics(B, 4, json!([]));
        assert_eq!(
            client.signature_help(B, 3, cursor + "false".len()),
            json!({"signatures":[pick_signature("n: Int", "tail: Boolean")],"activeParameter":3,"activeSignature":0})
        );
        client.shutdown();
    }
}

#[test]
fn changing_provider_visibility_refreshes_signature_help_in_an_unedited_document() {
    const OPS: &str = "file:///fixture/Ops.scala";
    let methods = format!("/* 😀 */ {PICK_OVERLOADS}");
    let line = "  def run(ops: Ops): Int = /* 😀 */ ops.pick(1, true, 2, ";
    let first = pick_signature("n: Int", "tail: Boolean");
    let second = pick_signature("flag: Boolean", "tail: Int");
    for enc in ["utf-8", "utf-16"] {
        let mut client = Client::start(enc, vec![OPS]);
        client.open(OPS, &methods);
        client.open(B, &format!("class Use {{\n{line}\n}}"));
        let edit = token_column(enc, &methods, "def pick");
        let before_third = token_column(enc, line, "true,") + "true,".len();
        let cursor = token_column(enc, line, "2, ") + "2, ".len();
        for version in 1..=3 {
            match version {
                2 => client.edit(OPS, version, range(0, edit, edit), "private "),
                3 => client.edit(OPS, version, range(0, edit, edit + "private ".len()), ""),
                _ => {}
            }
            expect_missing_argument(&mut client, 1);
            if version > 1 {
                client.diagnostics(OPS, version, json!([]));
            }
            let accessible = if version == 2 {
                vec![&second]
            } else {
                vec![&first, &second]
            };
            assert_eq!(
                client.signature_help(B, 1, before_third),
                json!({"signatures":accessible,"activeParameter":2})
            );
            let expected = if version == 2 {
                Value::Null
            } else {
                json!({"signatures":[&first],"activeParameter":3})
            };
            assert_eq!(
                client.signature_help(B, 1, cursor),
                expected,
                "{enc}, provider version {version}"
            );
        }
        client.shutdown();
    }
}

#[test]
fn named_signature_help_tracks_each_overload_through_edit_close_and_undo() {
    let methods = "class Ops { def pick(left: Int, right: Boolean): Int = left; def pick(right: Boolean, n: Int): Int = n }";
    let line = "  def run(o: Ops): Int = /* 😀 */ o.pick(";
    let signature = |a: &str, b: &str| json!({"label":format!("def pick({a}, {b}): Int"), "parameters":[{"label":a},{"label":b}]});
    let a = signature("left: Int", "right: Boolean");
    let b = signature("right: Boolean", "n: Int");
    for enc in ["utf-8", "utf-16"] {
        let mut client = Client::start(enc, vec![A]);
        client.open(A, methods);
        let mut previous = "right = ";
        client.open(B, &format!("class Use {{\r\n{line}{previous}\r\n}}"));
        let start = token_column(enc, line, "pick(") + 5;
        let mut named_a = a.clone();
        named_a["activeParameter"] = json!(1);
        let mut named_b = b.clone();
        named_b["activeParameter"] = json!(0);
        let alternatives = json!({"signatures":[named_a,named_b]});
        for (i, (args, expected)) in [
            ("right = ", alternatives.clone()),
            ("missing = ", Value::Null),
            ("right = ", alternatives.clone()),
            (
                "right = true, n = ",
                json!({"signatures":[b],"activeParameter":1}),
            ),
            (
                "right = true, left = ",
                json!({"signatures":[a],"activeParameter":0}),
            ),
            (
                "right = true, left = 9)",
                json!({"signatures":[a],"activeParameter":0,"activeSignature":0}),
            ),
            ("right = ", alternatives),
        ]
        .into_iter()
        .enumerate()
        {
            let version = i as i32 + 1;
            if i > 0 {
                client.edit(B, version, range(1, start, start + previous.len()), args);
            }
            let closed = args.ends_with(')');
            if closed {
                client.diagnostics(B, version, json!([]));
            } else {
                let Message::Notification(n) = client.receive() else {
                    panic!("expected diagnostics")
                };
                assert_eq!(n.method, "textDocument/publishDiagnostics");
                assert_eq!(n.params["uri"], B);
                assert_eq!(n.params["version"], version);
                assert!(
                    n.params["diagnostics"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|d| d["code"] == "parser.expression.missing_assignment_rhs"),
                    "{}",
                    n.params
                );
            }
            let cursor = start + args.len() - usize::from(closed);
            assert_eq!(
                client.signature_help(B, 1, cursor),
                expected,
                "{enc}: {args}"
            );
            assert_eq!(client.hover(B, 1, start - 3).is_null(), !closed);
            previous = args;
        }
        client.shutdown();
    }
}

#[test]
fn removing_a_provider_default_invalidates_the_unchanged_named_call_until_undo() {
    const OPS: &str = "file:///fixture/Ops.scala";
    let method = "  /* 😀 */ def diff(left: Int = 9, right: Int = 4): Int = left - right";
    let use_site = "  def run(o: Ops): Int = /* 😀 */ o.diff(right = 4)";
    for enc in ["utf-8", "utf-16"] {
        let mut client = Client::start(enc, vec![OPS]);
        client.open(OPS, &format!("class Ops {{\r\n{method}\r\n}}"));
        client.open(B, &format!("class Use {{\r\n{use_site}\r\n}}"));
        client.diagnostics(B, 1, json!([]));
        let cursor = token_column(enc, use_site, "right = 4") + 8;
        let call = token_column(enc, use_site, "o.diff");
        let edit = token_column(enc, method, " = 9");
        let help = json!({"activeSignature":0,"activeParameter":1,"signatures":[{"label":"def diff(left: Int = …, right: Int = …): Int","parameters":[{"label":"left: Int = …"},{"label":"right: Int = …"}]}]});
        assert_eq!(client.signature_help(B, 1, cursor), help);
        client.edit(OPS, 2, range(1, edit, edit + 4), "");
        client.diagnostics(B,1,json!([{"range":range(1,call,call+"o.diff(right = 4)".len()),"severity":1,"source":"dotty","code":"typer.missing_required_argument","message":"Missing argument for parameter left"}]));
        client.diagnostics(OPS, 2, json!([]));
        assert_eq!(client.signature_help(B, 1, cursor), Value::Null);
        client.edit(OPS, 3, range(1, edit, edit), " = 9");
        client.diagnostics(B, 1, json!([]));
        client.diagnostics(OPS, 3, json!([]));
        assert_eq!(client.signature_help(B, 1, cursor), help);
        client.shutdown();
    }
}

#[test]
fn editing_the_operand_retargets_only_the_named_argument_label() {
    let provider = [
        "class A {",
        "  def pick(/* 😀 */ right: Int): Int = right",
        "  def pick(n: Int = 9, /* 😀 */ right: Boolean): Int = if (right) n else 0",
        "}",
    ];
    let caller = [
        "class B {",
        "  def run(a: A): Int = { /* 😀 */ val right = 4;",
        "    /* 😀 */ a.pick(right = right) }",
        "}",
    ];
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, &provider.join("\n"));
        client.open(B, &caller.join("\n"));
        let col = |line: &str, token: &str| token_column(encoding, line, token);
        let local = col(caller[1], "right =");
        let init = col(caller[1], "4;");
        let label = col(caller[2], "right =");
        let rhs = col(caller[2], "right)");
        for (version, ty, literal, prev_len) in [
            (1, "Int", "4", 0),
            (2, "Boolean", "true", 1),
            (3, "Int", "4", 4),
        ] {
            if version != 1 {
                client.edit(B, version, range(1, init, init + prev_len), literal);
            }
            client.diagnostics(B, version, json!([]));
            for (line, formal_type, read) in [(1, "Int", "= right"), (2, "Boolean", "(right)")] {
                let decl = col(provider[line], "right:");
                let body = col(provider[line], read) + read.find("right").unwrap();
                let mut sites = vec![json!({"uri":A,"range":range(line,body,body+5)})];
                if formal_type == ty {
                    sites.push(json!({"uri":B,"range":range(2,label,label+5)}));
                    assert_eq!(client.references(B, 2, label, false), json!(sites));
                    let definition = client.request(
                        "textDocument/definition",
                        json!({"textDocument":{"uri":B},"position":{"line":2,"character":label+1}}),
                    );
                    assert_eq!(
                        definition,
                        json!([{"originSelectionRange":range(2,label,label+5),
                        "targetUri":A,"targetRange":range(line,decl,decl+7+ty.len()),
                        "targetSelectionRange":range(line,decl,decl+5)}])
                    );
                }
                assert_eq!(client.references(A, line, decl, false), json!(sites));
            }
            let definition = client.request(
                "textDocument/definition",
                json!({"textDocument":{"uri":B},"position":{"line":2,"character":rhs+1}}),
            );
            assert_eq!(
                definition,
                json!([{"originSelectionRange":range(2,rhs,rhs+5),
                "targetUri":B,"targetRange":range(1,local-4,init+literal.len()),
                "targetSelectionRange":range(1,local,local+5)}])
            );
            let local_uses = json!([{"uri":B,"range":range(2,rhs,rhs+5)}]);
            assert_eq!(client.references(B, 1, local, false), local_uses);
            assert_eq!(client.references(B, 2, rhs, false), local_uses);
            for start in [label, rhs] {
                assert_eq!(
                    client.hover(B, 2, start),
                    json!({"range":range(2,start,start+5),
                    "contents":{"kind":"plaintext","value":format!("right: {ty}")}})
                );
            }
        }
        client.shutdown();
    }
}

#[test]
fn rejected_overloads_publish_one_diagnostic_with_negotiated_candidate_locations() {
    let provider = [
        "class A {",
        "  def pick(/* 😀 */ right: Int): Int = right",
        "  def pick(/* 😀 */ flag: Boolean): Int = if (flag) 1 else 0",
        "}",
    ];
    let caller = "class B { def run(o: A): Int = /* 😀 */ o.pick(right = true) }";
    for (encoding, related) in [("utf-8", true), ("utf-16", true), ("utf-16", false)] {
        let mut client = Client::start_configured(encoding, vec![A], Value::Null, related);
        client.open(A, &provider.join("\n"));
        client.open(B, caller);
        let col = |line: &str, token: &str| token_column(encoding, line, token);
        let start = col(caller, "o.pick");
        let label = col(caller, "right =");
        let rhs = col(caller, "true");
        let int = col(provider[1], "right:");
        let flag = col(provider[2], "flag:");
        let pick = col(provider[2], "pick");
        let mut diagnostic = json!({"severity":1,"source":"dotty","code":"typer.no_applicable_overload","message":"No overload accepts these arguments","range":range(0,start,rhs+5)});
        if related {
            let location = |uri, line, start, len, msg| json!({"location":{"uri":uri,"range":range(line,start,start+len)},"message":msg});
            diagnostic["relatedInformation"] = json!([
                location(B, 0, rhs, 4, "Expected Int, found Boolean"),
                location(A, 1, int, 5, "Expected Int, found Boolean"),
                location(B, 0, label, 5, "Unknown parameter right"),
                location(A, 2, pick, 4, "Unknown parameter right"),
            ]);
        }
        let jump = json!({"textDocument":{"uri":B},"position":{"line":0,"character":label+1}});
        client.diagnostics(B, 1, json!([diagnostic]));
        assert_eq!(
            client.request("textDocument/definition", jump.clone()),
            Value::Null
        );
        client.edit(B, 2, range(0, label, label + 5), "flag");
        client.diagnostics(B, 2, json!([]));
        assert_eq!(
            client.request("textDocument/definition", jump.clone()),
            json!([{
            "originSelectionRange":range(0,label,label+4),"targetUri":A,
            "targetRange":range(2,flag,flag+13),"targetSelectionRange":range(2,flag,flag+4)}])
        );
        client.edit(B, 3, range(0, label, label + 4), "right");
        client.diagnostics(B, 3, json!([diagnostic]));
        assert_eq!(client.request("textDocument/definition", jump), Value::Null);
        client.shutdown();
    }
}

#[test]
fn named_constructor_signature_and_definition_survive_duplicate_label_repair() {
    let provider = "class Pair(left: Int, /* 😀 */ right: Int)";
    let text =
        "class Use { def make(right: Int): Pair = /* 😀 */ new Pair(right = right, left = 9) }";
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, provider);
        client.open(B, text);
        client.diagnostics(B, 1, json!([]));
        let col = |s| token_column(encoding, text, s);
        let first = col("right =");
        let second = col("left =");
        let rhs = col("right,");
        let formal = token_column(encoding, provider, "right:");
        for version in [1, 3] {
            if version == 3 {
                client.edit(B, 3, range(0, first, first + 4), "right");
                client.diagnostics(B, 3, json!([]));
            }
            for (cursor, active) in [(first, 1), (rhs, 1), (second, 0)] {
                assert_eq!(
                    client.signature_help(B, 0, cursor),
                    json!({
                        "signatures":[{"label":"new Pair(left: Int, right: Int)",
                        "parameters":[{"label":"left: Int"},{"label":"right: Int"}]}],
                        "activeSignature":0,"activeParameter":active
                    })
                );
            }
            for (cursor, target, start) in [(first, A, formal), (rhs, B, col("right:"))] {
                let def = client.request(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":B},"position":{"line":0,"character":cursor}}),
                );
                assert_eq!(def[0]["targetUri"], target);
                assert_eq!(def[0]["targetSelectionRange"], range(0, start, start + 5));
            }
            assert_eq!(client.hover(B, 0, first)["contents"]["value"], "right: Int");
            assert_eq!(
                client.references(A, 0, formal, false),
                json!([{"uri":B,"range":range(0,first,first+5)}])
            );
            if version == 1 {
                client.edit(B, 2, range(0, first, first + 5), "left");
                client.diagnostics(B, 2, json!([{"range":range(0,second-1,second+3),
                    "severity":1,"source":"dotty","code":"typer.duplicate_named_argument","message":"Parameter supplied twice"}]));
                assert_eq!(client.signature_help(B, 0, first), Value::Null);
                assert_eq!(client.references(A, 0, formal, false), json!([]));
            }
        }
        client.shutdown();
    }
}

#[test]
fn editing_a_constructor_field_to_val_preserves_named_allocation_and_reads() {
    const PROVIDER: &str = "class Counter(seed: Int, val id: Int, /* 😀 */ var count: Int)";
    let lines = [
        "class Use {",
        "  def make(count: Int): Counter = /* 😀 */ new Counter(count = count, seed = 9, id = 2)",
        "  def read(c: Counter): Int = /* 😀 */ c.count",
        "  def write(c: Counter): Unit = /* 😀 */ c.count = 7",
        "}",
    ];
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![]);
        client.open(A, PROVIDER);
        client.diagnostics(A, 1, json!([]));
        client.open(B, &lines.join("\n"));
        let col = |line, token| token_column(encoding, line, token);
        let modifier = col(PROVIDER, "var");
        let decl = col(PROVIDER, "count:");
        let first = col(lines[1], "count =");
        let rhs = col(lines[1], "count,");
        let read = col(lines[2], "c.count") + 2;
        let write = col(lines[3], "c.count") + 2;
        for (version, keyword) in [(1, "var"), (2, "val"), (3, "var")] {
            if version > 1 {
                client.edit(A, version, range(0, modifier, modifier + 3), keyword);
            }
            client.diagnostics(A, version, json!([]));
            let mutable = keyword == "var";
            let problems = if mutable {
                json!([])
            } else {
                json!([{
                    "range":range(3,write-2,write+5),"severity":1,"source":"dotty",
                    "message":"Cannot assign to an immutable value"
                }])
            };
            client.diagnostics(B, 1, problems);
            for (cursor, active) in [
                (first, 2),
                (col(lines[1], "seed ="), 0),
                (col(lines[1], "id ="), 1),
            ] {
                let sig = client.signature_help(B, 1, cursor);
                assert_eq!(sig["activeParameter"], active);
                assert_eq!(
                    sig["signatures"][0]["label"],
                    format!("new Counter(seed: Int, val id: Int, {keyword} count: Int)")
                );
            }
            for (line, cursor, uri, start) in [
                (1, first, A, decl),
                (2, read, A, decl),
                (1, rhs, B, col(lines[1], "count:")),
            ] {
                let def = client.request(
                    "textDocument/definition",
                    json!({"textDocument":{"uri":B},"position":{"line":line,"character":cursor}}),
                );
                assert_eq!(def[0]["targetUri"], uri);
                assert_eq!(
                    def[0]["targetSelectionRange"],
                    range(if uri == A { 0 } else { 1 }, start, start + 5)
                );
            }
            let mut uses = vec![
                json!({"uri":B,"range":range(1,first,first+5)}),
                json!({"uri":B,"range":range(2,read,read+5)}),
            ];
            if mutable {
                uses.push(json!({"uri":B,"range":range(3,write,write+5)}));
            }
            assert_eq!(client.references(A, 0, decl, false), json!(uses));
        }
        client.shutdown();
    }
}

const PARENT: &str = "class Base(val id: Int, /* 😀 */ var count: Int)";
const CHILD: &str =
    "class Child(id: Int, initial: Int) extends /* 😀 */ Base(count = initial, id = id)";

#[test]
fn parent_signature_and_definitions_follow_formals_in_both_encodings() {
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, PARENT);
        client.open(B, CHILD);
        client.diagnostics(B, 1, json!([]));
        let col = |text, token| token_column(encoding, text, token);
        for (cursor, ordinal) in [(col(CHILD, "count ="), 1), (col(CHILD, "id ="), 0)] {
            assert_eq!(
                client.signature_help(B, 0, cursor),
                json!({
                    "signatures":[{"label":"extends Base(val id: Int, var count: Int)",
                    "parameters":[{"label":"val id: Int"},{"label":"var count: Int"}]}],
                    "activeSignature":0,"activeParameter":ordinal
                })
            );
        }
        for (cursor, uri, start) in [
            (col(CHILD, "id ="), A, col(PARENT, "id:")),
            (col(CHILD, "id)"), B, col(CHILD, "id:")),
        ] {
            let def = client.request(
                "textDocument/definition",
                json!({"textDocument":{"uri":B},"position":{"line":0,"character":cursor}}),
            );
            assert_eq!(def[0]["targetUri"], uri);
            assert_eq!(def[0]["targetSelectionRange"], range(0, start, start + 2));
            assert_eq!(
                client.references(uri, 0, start, false),
                json!([{"uri": B, "range": range(0, cursor, cursor + 2)}])
            );
        }
        client.shutdown();
    }
}

#[test]
fn parent_label_diagnostic_tracks_a_provider_rename_and_consumer_repair() {
    for encoding in ["utf-8", "utf-16"] {
        let mut client = Client::start(encoding, vec![A]);
        client.open(A, PARENT);
        client.open(B, CHILD);
        client.diagnostics(B, 1, json!([]));
        let formal = token_column(encoding, PARENT, "count:");
        let label = token_column(encoding, CHILD, "count =");
        let definition = json!({"textDocument":{"uri":B},"position":{"line":0,"character":label}});
        client.edit(A, 2, range(0, formal, formal + 5), "total");
        client.diagnostics(A, 2, json!([]));
        client.diagnostics(
            B,
            1,
            json!([{
                "severity":1,"source":"dotty","code":"typer.unknown_named_argument",
                "message":"The selected clause has no parameter with this name.",
                "range":range(0,label,label+5)
            }]),
        );
        assert_eq!(client.signature_help(B, 0, label), Value::Null);
        assert_eq!(
            client.request("textDocument/definition", definition.clone()),
            Value::Null
        );
        assert_eq!(client.references(A, 0, formal, false), json!([]));

        client.edit(B, 2, range(0, label, label + 5), "total");
        client.diagnostics(A, 2, json!([]));
        client.diagnostics(B, 2, json!([]));
        let sig = client.signature_help(B, 0, label);
        assert_eq!(sig["activeParameter"], 1);
        assert_eq!(
            sig["signatures"][0]["label"],
            "extends Base(val id: Int, var total: Int)"
        );
        let def = client.request("textDocument/definition", definition);
        assert_eq!(def[0]["targetUri"], A);
        assert_eq!(def[0]["targetSelectionRange"], range(0, formal, formal + 5));
        assert_eq!(
            client.references(A, 0, formal, false),
            json!([{"uri": B, "range":range(0,label,label+5)}])
        );
        client.shutdown();
    }
}
