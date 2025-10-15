use std::{collections::HashMap, ffi::OsStr, path::Path, str::FromStr};

use anyhow::{anyhow, Context, Result};
use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionList, CompletionResponse,
    Diagnostic as LspDiagnostic, DiagnosticSeverity as LspDiagnosticSeverity, Documentation,
    NumberOrString, Position, PublishDiagnosticsParams, Range, Uri,
};
use zed_intellij_bridge::client::{
    BridgeClient, BridgeNotification, CompletionItemData, CompletionItemKindModel,
    CompletionResponseData, DiagnosticData, DiagnosticsNotification, GradleSyncResultData,
    WorkspaceEventNotification, WorkspaceOpenResultData,
};

pub use zed_intellij_bridge::{HandshakeAck, HandshakeHello};

/// Wraps an IntelliJ backend process and exposes high-level helpers for workspace/document
/// lifecycles and language service requests.
pub struct IntellijBackend {
    client: BridgeClient,
    documents: HashMap<(String, String), DocumentInfo>,
}

#[derive(Clone)]
struct DocumentInfo {
    uri: Uri,
}

/// Converted diagnostics ready to be consumed by existing LSP-based infrastructure.
#[derive(Debug, Clone)]
pub struct DiagnosticsUpdate {
    pub workspace_id: String,
    pub document_id: String,
    pub params: PublishDiagnosticsParams,
    pub full: bool,
}

/// Notifications emitted by the backend after conversion to LSP-compatible types.
#[derive(Debug, Clone)]
pub enum Notification {
    Diagnostics(DiagnosticsUpdate),
    WorkspaceEvent(WorkspaceEventNotification),
}

impl IntellijBackend {
    /// Launch the backend process using the provided executable/arguments and handshake payload.
    pub fn connect_with_hello<I, S>(
        executable: I,
        args: &[S],
        hello: HandshakeHello,
    ) -> Result<Self>
    where
        I: AsRef<Path>,
        S: AsRef<OsStr>,
    {
        let client = BridgeClient::connect(executable, args, hello)?;
        Ok(Self {
            client,
            documents: HashMap::new(),
        })
    }

    /// Convenience helper that uses the default handshake payload.
    pub fn connect<I, S>(executable: I, args: &[S]) -> Result<Self>
    where
        I: AsRef<Path>,
        S: AsRef<OsStr>,
    {
        Self::connect_with_hello(executable, args, HandshakeHello::default())
    }

    pub fn handshake(&self) -> &HandshakeAck {
        self.client.handshake()
    }

    pub fn open_workspace(
        &mut self,
        workspace_id: &str,
        root_uri: &str,
        project_type: &str,
    ) -> Result<WorkspaceOpenResultData> {
        self.client
            .open_workspace(workspace_id, root_uri, project_type)
    }

    pub fn close_workspace(&mut self, workspace_id: &str) -> Result<()> {
        self.client.close_workspace(workspace_id)
    }

    pub fn open_document(
        &mut self,
        workspace_id: &str,
        document_id: &str,
        uri: &str,
        language_id: &str,
        text: &str,
    ) -> Result<DiagnosticsUpdate> {
        let parsed_uri =
            Uri::from_str(uri).with_context(|| format!("invalid document URI: {uri}"))?;
        let notification =
            self.client
                .open_document(workspace_id, document_id, uri, language_id, text)?;
        self.documents.insert(
            (workspace_id.to_string(), document_id.to_string()),
            DocumentInfo {
                uri: parsed_uri.clone(),
            },
        );
        Ok(convert_diagnostics(notification, parsed_uri))
    }

    pub fn change_document(
        &mut self,
        workspace_id: &str,
        document_id: &str,
        new_text: &str,
    ) -> Result<DiagnosticsUpdate> {
        let info = self
            .documents
            .get(&(workspace_id.to_string(), document_id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow!("unknown document {document_id}"))?;
        let notification = self
            .client
            .change_document(workspace_id, document_id, new_text)?;
        Ok(convert_diagnostics(notification, info.uri))
    }

    pub fn close_document(&mut self, workspace_id: &str, document_id: &str) -> Result<()> {
        self.documents
            .remove(&(workspace_id.to_string(), document_id.to_string()));
        self.client.close_document(workspace_id, document_id)
    }

    pub fn request_completion(
        &mut self,
        workspace_id: &str,
        document_id: &str,
        line: i32,
        character: i32,
    ) -> Result<CompletionResponse> {
        let response =
            self.client
                .request_completion(workspace_id, document_id, line, character)?;
        Ok(convert_completion_response(response))
    }

    pub fn run_gradle_sync(
        &mut self,
        workspace_id: &str,
        project_root: &str,
        arguments: &[String],
    ) -> Result<GradleSyncResultData> {
        self.client
            .run_gradle_sync(workspace_id, project_root, arguments)
    }

    pub fn poll_notification(&mut self) -> Option<Notification> {
        while let Some(notification) = self.client.poll_notification() {
            match notification {
                BridgeNotification::Diagnostics(diag) => {
                    if let Some(info) = self
                        .documents
                        .get(&(diag.workspace_id.clone(), diag.document_id.clone()))
                        .cloned()
                    {
                        return Some(Notification::Diagnostics(convert_diagnostics(
                            diag, info.uri,
                        )));
                    }
                }
                BridgeNotification::WorkspaceEvent(event) => {
                    return Some(Notification::WorkspaceEvent(event));
                }
            }
        }
        None
    }

    pub fn wait(self) -> Result<std::process::ExitStatus> {
        self.client.wait()
    }
}

fn convert_diagnostics(notification: DiagnosticsNotification, uri: Uri) -> DiagnosticsUpdate {
    let DiagnosticsNotification {
        workspace_id,
        document_id,
        version,
        full,
        diagnostics,
    } = notification;
    let diagnostics = diagnostics.into_iter().map(convert_diagnostic).collect();
    let params = PublishDiagnosticsParams::new(uri, diagnostics, Some(version));
    DiagnosticsUpdate {
        workspace_id,
        document_id,
        params,
        full,
    }
}

fn convert_diagnostic(diagnostic: DiagnosticData) -> LspDiagnostic {
    LspDiagnostic {
        range: convert_range(&diagnostic.range),
        severity: map_severity(diagnostic.severity),
        code: diagnostic.code.map(NumberOrString::String),
        code_description: None,
        source: diagnostic.source,
        message: diagnostic.message,
        related_information: None,
        tags: None,
        data: None,
        ..LspDiagnostic::default()
    }
}

fn map_severity(
    severity: zed_intellij_bridge::client::DiagnosticSeverityModel,
) -> Option<LspDiagnosticSeverity> {
    use zed_intellij_bridge::client::DiagnosticSeverityModel as BridgeSeverity;
    match severity {
        BridgeSeverity::Unknown => None,
        BridgeSeverity::Error => Some(LspDiagnosticSeverity::ERROR),
        BridgeSeverity::Warning => Some(LspDiagnosticSeverity::WARNING),
        BridgeSeverity::Information => Some(LspDiagnosticSeverity::INFORMATION),
        BridgeSeverity::Hint => Some(LspDiagnosticSeverity::HINT),
    }
}

fn convert_range(range: &zed_intellij_bridge::client::RangeData) -> Range {
    Range {
        start: convert_position(&range.start),
        end: convert_position(&range.end),
    }
}

fn convert_position(position: &zed_intellij_bridge::client::PositionData) -> Position {
    Position {
        line: position.line.max(0) as u32,
        character: position.character.max(0) as u32,
    }
}

fn convert_completion_response(response: CompletionResponseData) -> CompletionResponse {
    let items: Vec<CompletionItem> = response
        .items
        .into_iter()
        .map(convert_completion_item)
        .collect();
    CompletionResponse::List(CompletionList {
        is_incomplete: response.is_incomplete,
        item_defaults: None,
        items,
    })
}

fn convert_completion_item(item: CompletionItemData) -> CompletionItem {
    CompletionItem {
        label: item.label,
        detail: item.detail,
        documentation: item.documentation.map(Documentation::String),
        insert_text: item.insert_text,
        kind: item.kind.and_then(map_completion_item_kind),
        ..CompletionItem::default()
    }
}

fn map_completion_item_kind(kind: CompletionItemKindModel) -> Option<CompletionItemKind> {
    use CompletionItemKindModel::*;
    Some(match kind {
        Unknown => return None,
        Text => CompletionItemKind::TEXT,
        Method => CompletionItemKind::METHOD,
        Function => CompletionItemKind::FUNCTION,
        Constructor => CompletionItemKind::CONSTRUCTOR,
        Field => CompletionItemKind::FIELD,
        Variable => CompletionItemKind::VARIABLE,
        Class => CompletionItemKind::CLASS,
        Interface => CompletionItemKind::INTERFACE,
        Module => CompletionItemKind::MODULE,
        Property => CompletionItemKind::PROPERTY,
        Value => CompletionItemKind::VALUE,
        Enum => CompletionItemKind::ENUM,
        Keyword => CompletionItemKind::KEYWORD,
        Snippet => CompletionItemKind::SNIPPET,
        Color => CompletionItemKind::COLOR,
        File => CompletionItemKind::FILE,
        Reference => CompletionItemKind::REFERENCE,
        Folder => CompletionItemKind::FOLDER,
        EnumMember => CompletionItemKind::ENUM_MEMBER,
        Constant => CompletionItemKind::CONSTANT,
        Struct => CompletionItemKind::STRUCT,
        Event => CompletionItemKind::EVENT,
        Operator => CompletionItemKind::OPERATOR,
        TypeParameter => CompletionItemKind::TYPE_PARAMETER,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zed_intellij_bridge::client::{DiagnosticSeverityModel, PositionData, RangeData};

    #[test]
    fn diagnostics_convert_to_publish_params() {
        let notification = DiagnosticsNotification {
            workspace_id: "ws".into(),
            document_id: "doc".into(),
            version: 3,
            full: true,
            diagnostics: vec![DiagnosticData {
                range: RangeData {
                    start: PositionData {
                        line: 1,
                        character: 2,
                    },
                    end: PositionData {
                        line: 1,
                        character: 5,
                    },
                },
                severity: DiagnosticSeverityModel::Warning,
                code: Some("W123".into()),
                message: "Something happened".into(),
                source: Some("intellij".into()),
            }],
        };
        let uri = Uri::from_str("file:///tmp/Main.kt").unwrap();
        let update = convert_diagnostics(notification, uri.clone());
        assert_eq!(update.params.uri, uri);
        assert_eq!(update.params.version, Some(3));
        assert!(update.full);
        let diag = update.params.diagnostics.first().unwrap();
        assert_eq!(diag.range.start.line, 1);
        assert_eq!(diag.range.start.character, 2);
        assert_eq!(diag.severity, Some(LspDiagnosticSeverity::WARNING));
        assert_eq!(
            diag.code.as_ref().unwrap(),
            &NumberOrString::String("W123".into())
        );
    }

    #[test]
    fn completion_convert_to_lsp_items() {
        let response = CompletionResponseData {
            items: vec![CompletionItemData {
                label: "println".into(),
                detail: Some("fun".into()),
                documentation: Some("Prints a line".into()),
                insert_text: Some("println()".into()),
                kind: Some(CompletionItemKindModel::Function),
            }],
            is_incomplete: false,
        };
        let converted = convert_completion_response(response);
        match converted {
            CompletionResponse::List(list) => {
                assert_eq!(list.items.len(), 1);
                let item = &list.items[0];
                assert_eq!(item.label, "println");
                assert_eq!(item.detail.as_deref(), Some("fun"));
                assert!(matches!(item.kind, Some(CompletionItemKind::FUNCTION)));
                assert_eq!(item.insert_text.as_deref(), Some("println()"));
            }
            _ => panic!("expected completion list"),
        }
    }
}
