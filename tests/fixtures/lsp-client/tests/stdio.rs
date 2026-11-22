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
            "}\r\n"
        ),
    );
    client.diagnostics(B, 1, json!([]));
    client.definition(take_column, 1, INT_METHOD);

    client.edit(B, 2, range(1, take_column + 5, take_column + 7), "false");
    client.diagnostics(B, 2, json!([]));
    client.definition(take_column, 2, BOOL_METHOD);

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

    client.edit(B, 4, range(3, 24, 29), "1");
    client.diagnostics(B, 4, json!([]));
    client.definition(take_column, 2, BOOL_METHOD);
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
fn parser_then_namer_diagnostics_track_unsaved_edits_in_both_encodings() {
    let uri = "file:///fixture/Outer.scala";
    for (encoding, literal, qualifier) in [("utf-8", 46, 21), ("utf-16", 44, 19)] {
        let mut client = Client::start(encoding, vec![]);
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
        client.edit(uri, 2, range(1, literal, literal), "= ");
        client.diagnostics(uri, 2, json!([{
            "source": "dotty", "severity": 1, "range": range(1, qualifier, qualifier + 7),
            "code": "namer.visibility.invalid_qualifier",
            "message": "Invalid 'private' qualifier 'Missing': expected an enclosing class, object or package"
        }]));
        client.edit(uri, 3, range(1, qualifier - 1, qualifier + 8), "");
        client.diagnostics(uri, 3, json!([]));
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
        client.open(uri, text);
        client.diagnostics(uri, 1, json!([warning, error]));
        client.edit(uri, 2, range(4, boolean, boolean + 5), "42");
        client.diagnostics(uri, 2, json!([warning]));
        client.edit(uri, 3, range(3, one, one), "using ");
        client.diagnostics(uri, 3, json!([]));
        client.shutdown();
    }
}
