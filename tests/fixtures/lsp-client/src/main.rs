use crossbeam_channel::{Receiver, Sender, select, unbounded};
use dotty::core::{DiagnosticSeverity, DocumentText, PositionEncoding, TextRange};
use dotty::{
    ResolvedCallSignature, SourceDiagnostic, SourceSignatureParameter, SourceSymbolInfo,
    SourceSymbolSignature,
};
use dotty_lsp::analysis::{Analysis, Document, Documents, Problem, analyze_with_classpath};
use lsp_server::{Connection, Message, Notification, Request, Response};
use serde_json::{Value, json};
use std::{error::Error, sync::Arc, thread};

type Job = (u64, Documents);
type Completed = (u64, Analysis);

fn wire_range(text: &DocumentText, r: TextRange, enc: PositionEncoding) -> Option<Value> {
    let (sl, sc) = text.position(r.start(), enc)?;
    let (el, ec) = text.position(r.end(), enc)?;
    Some(json!({"start":{"line":sl,"character":sc},"end":{"line":el,"character":ec}}))
}
fn parameter_labels(parameters: &[SourceSignatureParameter]) -> Vec<String> {
    parameters
        .iter()
        .map(|p| format!("{}: {}", p.name, p.ty.name))
        .collect()
}

fn method_label(name: &str, labels: Option<&[String]>, result: &str) -> String {
    match labels {
        Some(labels) => format!("def {name}({}): {result}", labels.join(", ")),
        None => format!("def {name}: {result}"),
    }
}

fn signature_information(
    name: &str,
    parameters: &[SourceSignatureParameter],
    result: &str,
) -> Value {
    let labels = parameter_labels(parameters);
    let method = method_label(name, Some(&labels), result);
    let parameters: Vec<_> = labels
        .into_iter()
        .map(|label| json!({"label":label}))
        .collect();
    json!({"label":method,"parameters":parameters})
}

fn signature_help(call: ResolvedCallSignature<'_>) -> Value {
    let selected = signature_information(call.name, call.parameters, &call.result.name);
    let mut help = json!({"signatures":[selected],"activeSignature":0});
    if let Some(index) = call.active_parameter {
        help["activeParameter"] = json!(index);
    }
    help
}

fn hover_label(info: &SourceSymbolInfo) -> String {
    let name = &info.name;
    match &info.signature {
        SourceSymbolSignature::Class => format!("class {name}"),
        SourceSymbolSignature::Value(ty) => format!("{name}: {}", ty.name),
        SourceSymbolSignature::Method {
            parameters, result, ..
        } => {
            let labels = parameters.as_deref().map(parameter_labels);
            method_label(name, labels.as_deref(), &result.name)
        }
    }
}

fn offset(text: &DocumentText, p: &Value, enc: PositionEncoding) -> Option<u32> {
    text.offset(
        p["line"].as_u64()?.try_into().ok()?,
        p["character"].as_u64()?.try_into().ok()?,
        enc,
    )
}
fn apply_changes(
    doc: &Document,
    changes: &[Value],
    enc: PositionEncoding,
) -> Option<Arc<DocumentText>> {
    let mut current = doc.text.clone();
    for change in changes {
        let input = change["text"].as_str()?;
        let text = if let Some(range) = change.get("range") {
            let start = offset(&current, &range["start"], enc)? as usize;
            let end = offset(&current, &range["end"], enc)? as usize;
            if start > end {
                return None;
            }
            let mut s = current.as_str().to_owned();
            s.replace_range(start..end, input);
            s
        } else {
            input.to_owned()
        };
        current = Arc::new(DocumentText::new(text).ok()?);
    }
    Some(current)
}

#[derive(Default)]
struct Workspace {
    docs: Documents,
    revision: u64,
    ready: Option<Analysis>,
}

impl Workspace {
    fn changed(&mut self, method: &str, p: &Value, enc: PositionEncoding) -> bool {
        let td = &p["textDocument"];
        let Some(uri) = td["uri"].as_str() else {
            return false;
        };
        if method == "textDocument/didClose" {
            if self.docs.remove(uri).is_none() {
                return false;
            }
        } else {
            let Some(version) = td["version"].as_i64().and_then(|n| i32::try_from(n).ok()) else {
                return false;
            };
            let text = match method {
                "textDocument/didOpen" => {
                    if self.docs.contains_key(uri) {
                        return false;
                    }
                    let Some(t) = td["text"]
                        .as_str()
                        .and_then(|t| DocumentText::new(t.to_owned()).ok())
                    else {
                        return false;
                    };
                    Arc::new(t)
                }
                "textDocument/didChange" => {
                    let Some(d) = self.docs.get(uri) else {
                        return false;
                    };
                    if version <= d.version {
                        return false;
                    }
                    let Some(t) = p["contentChanges"]
                        .as_array()
                        .and_then(|changes| apply_changes(d, changes, enc))
                    else {
                        return false;
                    };
                    t
                }
                _ => return false,
            };
            self.docs.insert(uri.to_owned(), Document { version, text });
        }
        self.revision += 1;
        self.ready = None;
        true
    }

    // A change in any open document can redirect a call in any other file.
    fn accept(&mut self, (revision, result): Completed) -> bool {
        if revision != self.revision {
            return false;
        }
        self.ready = Some(result);
        true
    }

    fn navigation(
        &self,
        method: &str,
        p: &Value,
        enc: PositionEncoding,
        links: bool,
    ) -> Option<Value> {
        let uri = p["textDocument"]["uri"].as_str()?;
        let text = &self.docs.get(uri)?.text;
        let pos = offset(text, &p["position"], enc)?;
        let analysis = self.ready.as_ref()?;
        let source = analysis.source(uri)?;
        if method == "textDocument/signatureHelp" {
            if let Some(call) = analysis.members.call_signature_at(source, pos) {
                return Some(signature_help(call));
            }
            let pending = analysis.unfinished.at(source, pos)?;
            let first = pending.candidates.first()?.active_parameter;
            let shared = pending
                .candidates
                .iter()
                .all(|c| c.active_parameter == first);
            let signatures: Vec<_> = pending
                .candidates
                .iter()
                .map(|c| {
                    let mut signature =
                        signature_information(&c.name, &c.parameters, &c.result.name);
                    if !shared {
                        signature["activeParameter"] = json!(c.active_parameter);
                    }
                    signature
                })
                .collect();
            let mut help = json!({"signatures":signatures});
            if shared {
                help["activeParameter"] = json!(first);
            }
            return Some(help);
        }
        if method == "textDocument/hover" {
            let (site, info) = analysis.members.symbol_info_at(source, pos)?;
            let token = site
                .usage
                .map_or(site.target.name, |usage| usage.span().range());
            return Some(
                json!({"contents":{"kind":"plaintext","value":hover_label(info)},
                "range":wire_range(text, token, enc)?}),
            );
        }
        let site = analysis.members.symbol_at(source, pos)?;
        let target = site.target;
        if method == "textDocument/references" {
            let location = |file, range| {
                let uri = analysis.uri(file)?;
                Some(json!({"uri":uri, "range":wire_range(&self.docs.get(uri)?.text, range, enc)?}))
            };
            let mut locations = Vec::new();
            if p["context"]["includeDeclaration"].as_bool()? {
                locations.push(location(target.source, target.name)?);
            }
            for reference in analysis.members.references_to(site.symbol) {
                locations.push(location(reference.source(), reference.span().range())?);
            }
            return Some(Value::Array(locations));
        }
        let origin = site.usage?;
        let target_uri = analysis.uri(target.source)?;
        let target_text = &self.docs.get(target_uri)?.text;
        let name = wire_range(target_text, target.name, enc)?;
        if links {
            Some(
                json!([{"originSelectionRange":wire_range(text,origin.span().range(),enc)?,
                "targetUri":target_uri,"targetRange":wire_range(target_text,target.declaration,enc)?,"targetSelectionRange":name}]),
            )
        } else {
            Some(json!({"uri":target_uri,"range":name}))
        }
    }

    fn publish(&self, conn: &Connection, enc: PositionEncoding) -> Result<(), Box<dyn Error>> {
        if let Some(files) = &self.ready {
            for (uri, file) in &files.files {
                let doc = &self.docs[uri];
                let mut diagnostics = Vec::new();
                for problem in &file.problems {
                    let report = match problem {
                        Problem::Semantic(diagnostic) => &render_semantic(diagnostic),
                        Problem::Preparation(d) => &d.render(),
                        Problem::Sources(error) => &unlocated_error(error),
                        Problem::Scan(error) => &unlocated_error(error),
                        Problem::Cycle { cycle, location } => &SourceDiagnostic {
                            code: Some(cycle.code()),
                            location: *location,
                            severity: DiagnosticSeverity::Error,
                            message: match cycle.phase() {
                                dotty::SignaturePhase::Headers => "Cyclic class header dependency",
                                dotty::SignaturePhase::Members => {
                                    "Cyclic member signature dependency"
                                }
                            }
                            .into(),
                        },
                        Problem::Pending { name, location, .. } => &SourceDiagnostic {
                            code: None,
                            location: *location,
                            severity: DiagnosticSeverity::Error,
                            message: format!("Waiting for the signature of '{name}'"),
                        },
                    };
                    publish_problem(conn, uri, &doc.text, enc, report, &mut diagnostics)?;
                }
                notify(conn, uri, Some(doc.version), diagnostics)?;
            }
        }
        Ok(())
    }
}

fn render_semantic(report: &dotty::SemanticDiagnostic) -> SourceDiagnostic {
    match &report.issue {
        dotty::SemanticIssue::Typing(error) => error.render(),
        dotty::SemanticIssue::Warning { source, diagnostic } => {
            SourceDiagnostic::from_typer_warning(*source, diagnostic)
        }
        dotty::SemanticIssue::InvalidClass(id)
            if report.phase == dotty::SemanticPhase::ClassBody =>
        {
            unlocated_error(&dotty::ClassBodyError::InvalidClass(*id))
        }
        dotty::SemanticIssue::InvalidClass(id) => {
            unlocated_error(&dotty::ClassHeaderError::InvalidClass(*id))
        }
        dotty::SemanticIssue::InvalidMember(id) => {
            unlocated_error(&dotty::MemberSignatureError::InvalidMember(*id))
        }
    }
}

fn unlocated_error(error: &dyn Error) -> SourceDiagnostic {
    SourceDiagnostic {
        code: None,
        location: None,
        severity: DiagnosticSeverity::Error,
        message: error.to_string(),
    }
}

fn publish_problem(
    conn: &Connection,
    uri: &str,
    text: &DocumentText,
    enc: PositionEncoding,
    problem: &SourceDiagnostic,
    diagnostics: &mut Vec<Value>,
) -> Result<(), Box<dyn Error>> {
    let severity = match problem.severity {
        DiagnosticSeverity::Error => 1,
        DiagnosticSeverity::Warning => 2,
        DiagnosticSeverity::Note => 3,
    };
    if let Some(range) = problem
        .location
        .and_then(|s| wire_range(text, s.span().range(), enc))
    {
        let mut diagnostic = json!({
            "range": range, "severity": severity,
            "source": "dotty", "message": problem.message
        });
        if let Some(code) = problem.code {
            diagnostic["code"] = json!(code);
        }
        diagnostics.push(diagnostic);
    } else {
        // LSP document diagnostics require a range. Keep unlocated errors visible.
        let params = json!({"type": severity, "message": format!("{uri}: {}", problem.message)});
        conn.sender
            .send(Notification::new("window/logMessage".into(), params).into())?;
    }
    Ok(())
}

fn notify(
    conn: &Connection,
    uri: &str,
    version: Option<i32>,
    diagnostics: Vec<Value>,
) -> Result<(), Box<dyn Error>> {
    let mut p = json!({"uri":uri,"diagnostics":diagnostics});
    if let Some(v) = version {
        p["version"] = json!(v);
    }
    conn.sender
        .send(Notification::new("textDocument/publishDiagnostics".into(), p).into())?;
    Ok(())
}

fn worker(
    jobs: Receiver<Job>,
    done: Sender<Completed>,
    classpath: Option<dotty::classloader::CompositeClassPath>,
) {
    while let Ok((revision, docs)) = jobs.recv() {
        if done
            .send((revision, analyze_with_classpath(&docs, classpath.as_ref())))
            .is_err()
        {
            break;
        }
    }
}

fn serve(conn: &Connection) -> Result<(), Box<dyn Error>> {
    let (id, params) = conn.initialize_start()?;
    let enc = params
        .pointer("/capabilities/general/positionEncodings")
        .and_then(Value::as_array)
        .and_then(|choices| {
            choices.iter().find_map(|v| match v.as_str()? {
                "utf-8" => Some(PositionEncoding::Utf8),
                "utf-16" => Some(PositionEncoding::Utf16),
                _ => None,
            })
        })
        .unwrap_or(PositionEncoding::Utf16);
    let links = params
        .pointer("/capabilities/textDocument/definition/linkSupport")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let encoding = match enc {
        PositionEncoding::Utf16 => "utf-16",
        PositionEncoding::Utf8 => "utf-8",
    };
    let classpath = match params.pointer("/initializationOptions/classDirectory") {
        Some(value) => {
            use dotty::classloader::{CompositeClassPath, DirectoryClassPath, JmodClassPath};
            let classes = value
                .as_str()
                .ok_or("classDirectory must be a directory path")?;
            let jdk =
                std::env::var_os("JAVA_HOME").ok_or("set JAVA_HOME when using classDirectory")?;
            Some(CompositeClassPath::new(vec![
                Box::new(DirectoryClassPath::new(classes)),
                Box::new(JmodClassPath::new(
                    std::path::Path::new(&jdk).join("jmods/java.base.jmod"),
                )?),
            ]))
        }
        None => None,
    };
    conn.initialize_finish(
        id,
        json!({
            "capabilities": {
                "positionEncoding": encoding,
                "definitionProvider": true,
                "hoverProvider": true,
                "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                "referencesProvider": true,
                "textDocumentSync": {"openClose": true, "change": 2}
            }
        }),
    )?;
    let mut ws = Workspace {
        ready: Some(Analysis::default()),
        ..Workspace::default()
    };
    let (send, jobs) = crossbeam_channel::bounded(1);
    let queued = jobs.clone();
    let (done, completed) = unbounded();
    let handle = thread::spawn(move || worker(jobs, done, classpath));
    let mut pending: Vec<Request> = Vec::new();
    enum Event {
        Input(Message),
        Completed(Completed),
    }
    loop {
        let event = select! {
            recv(completed) -> result => Event::Completed(result?),
            recv(conn.receiver) -> message => Event::Input(message?),
        };
        match event {
            Event::Completed(result) => {
                if ws.accept(result) {
                    ws.publish(conn, enc)?;
                    for req in pending.drain(..) {
                        conn.sender.send(
                            Response::new_ok(
                                req.id,
                                ws.navigation(&req.method, &req.params, enc, links),
                            )
                            .into(),
                        )?;
                    }
                }
            }
            Event::Input(message) => match message {
                Message::Request(req) => {
                    if conn.handle_shutdown(&req)? {
                        break;
                    }
                    if !matches!(
                        req.method.as_str(),
                        "textDocument/definition"
                            | "textDocument/references"
                            | "textDocument/hover"
                            | "textDocument/signatureHelp"
                    ) {
                        conn.sender.send(
                            Response::new_err(req.id, -32601, "Unsupported method".into()).into(),
                        )?;
                    } else if ws.ready.is_some() {
                        conn.sender.send(
                            Response::new_ok(
                                req.id,
                                ws.navigation(&req.method, &req.params, enc, links),
                            )
                            .into(),
                        )?;
                    } else {
                        pending.push(req);
                    }
                }
                Message::Notification(n) => {
                    if n.method == "exit" {
                        return Err("exit received without shutdown".into());
                    }
                    if n.method == "$/cancelRequest" {
                        if let Ok(id) =
                            serde_json::from_value::<lsp_server::RequestId>(n.params["id"].clone())
                            && let Some(i) = pending.iter().position(|r| r.id == id)
                        {
                            let r = pending.remove(i);
                            conn.sender.send(
                                Response::new_err(r.id, -32800, "Request cancelled".into()).into(),
                            )?;
                        }
                    } else if ws.changed(&n.method, &n.params, enc) {
                        for req in pending.drain(..) {
                            conn.sender.send(
                                Response::new_err(req.id, -32801, "Document changed".into()).into(),
                            )?;
                        }
                        if n.method == "textDocument/didClose"
                            && let Some(uri) = n.params["textDocument"]["uri"].as_str()
                        {
                            notify(conn, uri, None, vec![])?;
                        }
                        // Keep at most one waiting rebuild beside the one already running.
                        let _ = queued.try_recv();
                        send.send((ws.revision, ws.docs.clone()))?;
                    }
                }
                Message::Response(_) => {}
            },
        }
    }
    drop(send);
    handle.join().map_err(|_| "analysis worker failed")?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let (connection, threads) = Connection::stdio();
    serve(&connection)?;
    drop(connection);
    threads.join()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dotty_lsp::analysis::FileResult;
    use std::collections::BTreeMap;

    #[test]
    fn unlocated_problem_is_logged_without_a_document_marker() {
        let uri = "file:///A.scala";
        let mut ws = Workspace::default();
        let enc = PositionEncoding::Utf16;
        let open = json!({"textDocument":{"uri":uri,"version":1,"text":"class A {}"}});
        assert!(ws.changed("textDocument/didOpen", &open, enc));
        let file = FileResult {
            problems: vec![Problem::Semantic(dotty::SemanticDiagnostic {
                phase: dotty::SemanticPhase::MemberSignature,
                class: None,
                owner_source: None,
                issue: dotty::SemanticIssue::Typing(Box::new(
                    dotty::ReportedTyperError::from_typer(
                        dotty::typer::TyperError::ApplicationArityMismatch {
                            source: dotty::core::SourceId::from_index(0),
                            tree_index: 0,
                            expected: 0,
                            actual: 1,
                        },
                        |_| None,
                    ),
                )),
            })],
        };
        let mut analysis = Analysis::default();
        analysis.files.insert(uri.into(), file);
        assert!(ws.accept((ws.revision, analysis)));
        let (server, client) = Connection::memory();
        ws.publish(&server, enc).unwrap();
        let mut notifications: BTreeMap<_, _> = client
            .receiver
            .try_iter()
            .map(|message| {
                let Message::Notification(n) = message else {
                    panic!("expected notification")
                };
                (n.method, n.params)
            })
            .collect();
        assert_eq!(
            notifications.remove("window/logMessage"),
            Some(json!({
                "type": 1, "message":"file:///A.scala: Expected 0 arguments, got 1"
            }))
        );
        assert_eq!(
            notifications.remove("textDocument/publishDiagnostics"),
            Some(json!({
                "uri":uri, "version":1, "diagnostics":[]
            }))
        );
        assert!(notifications.is_empty());
    }

    #[test]
    fn late_results_cannot_replace_current_diagnostics_or_definitions() {
        let mut ws = Workspace::default();
        let id = "file:///B.scala";
        let enc = PositionEncoding::Utf16;
        let open = json!({"textDocument":{"uri":id,"version":1,"text":"😀ab"}});
        assert!(ws.changed("textDocument/didOpen", &open, enc));
        let edit = json!({"textDocument":{"uri":id,"version":2},"contentChanges":[
            {"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":3}},"text":"long"},
            {"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":7}},"text":"!"}]});
        assert!(ws.changed("textDocument/didChange", &edit, enc));
        assert_eq!(ws.docs[id].text.as_str(), "😀long!");
        assert!(!ws.changed("textDocument/didChange", &edit, enc));
        let mut analysis = Analysis::default();
        analysis.files.insert(id.into(), FileResult::default());
        assert!(ws.accept((2, analysis)));
        assert!(!ws.accept((1, Analysis::default())));
        assert!(ws.ready.unwrap().files.contains_key(id));
    }
}
