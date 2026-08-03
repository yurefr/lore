use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    application::protocol::{ProtocolFailure, ProtocolRequest, ProtocolResponse, ProtocolService},
    error::Result,
};

pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";
const SUPPORTED_MCP_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

#[derive(Clone)]
pub struct McpServer {
    protocol: ProtocolService,
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    protocol_version: String,
    #[serde(rename = "clientInfo")]
    client_info: ClientInfo,
    #[serde(default)]
    #[allow(dead_code)]
    capabilities: Value,
    #[serde(rename = "_meta", default)]
    metadata: Option<InitializeMetadata>,
}

#[derive(Debug, Deserialize)]
struct InitializeMetadata {
    #[serde(rename = "loreCapabilities", default)]
    lore_capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ClientInfo {
    name: String,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ToolCallParams {
    name: String,
    #[serde(default = "empty_object")]
    arguments: Value,
}

impl McpServer {
    pub fn new(protocol: ProtocolService) -> Self {
        Self { protocol }
    }

    pub fn serve_stdio(&self) -> Result<()> {
        let stdin = io::stdin();
        let mut stdout = io::BufWriter::new(io::stdout());
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(response) = self.handle_line(&line)? {
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
            }
        }
        Ok(())
    }

    pub fn handle_line(&self, input: &str) -> Result<Option<String>> {
        let request = match serde_json::from_str::<JsonRpcRequest>(input) {
            Ok(request) => request,
            Err(error) => {
                return Ok(Some(serialize_response(JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: "Parse error".into(),
                        data: Some(json!({ "detail": error.to_string() })),
                    }),
                })?));
            }
        };

        if request.method == "notifications/initialized" {
            return Ok(None);
        }

        let is_notification = request.id.is_none();
        let id = request.id.unwrap_or(Value::Null);
        let response = if request.jsonrpc != "2.0" {
            JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32600,
                    message: "Invalid Request".into(),
                    data: None,
                }),
            }
        } else {
            match self.dispatch(&request.method, request.params) {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: Some(result),
                    error: None,
                },
                Err(error) => JsonRpcResponse {
                    jsonrpc: "2.0",
                    id,
                    result: None,
                    error: Some(error),
                },
            }
        };

        if is_notification {
            Ok(None)
        } else {
            Ok(Some(serialize_response(response)?))
        }
    }

    fn dispatch(&self, method: &str, params: Value) -> std::result::Result<Value, JsonRpcError> {
        match method {
            "initialize" => self.initialize(params),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => self.call_tool(params),
            "ping" => Ok(json!({})),
            _ => Err(JsonRpcError {
                code: -32601,
                message: format!("Method not found: {method}"),
                data: None,
            }),
        }
    }

    fn initialize(&self, params: Value) -> std::result::Result<Value, JsonRpcError> {
        let params: InitializeParams = serde_json::from_value(params)
            .map_err(|error| invalid_params(format!("invalid initialize params: {error}")))?;
        if !SUPPORTED_MCP_PROTOCOL_VERSIONS.contains(&params.protocol_version.as_str()) {
            let failure = ProtocolFailure {
                code: crate::application::protocol::ProtocolErrorCode::UnsupportedProtocolVersion,
                message: format!(
                    "unsupported MCP protocol version {}; supported versions are {:?}",
                    params.protocol_version, SUPPORTED_MCP_PROTOCOL_VERSIONS
                ),
                details: None,
            };
            return Err(invalid_params_with_data(
                "unsupported protocol version",
                serde_json::to_value(failure).unwrap_or_else(|_| json!({})),
            ));
        }

        let capabilities = params
            .metadata
            .map(|metadata| metadata.lore_capabilities)
            .filter(|values| !values.is_empty())
            .unwrap_or_else(|| vec!["event_ingest".into()]);
        let request = ProtocolRequest::Handshake {
            protocol_version: crate::domain::event::CURRENT_PROTOCOL_VERSION,
            client_id: params.client_info.name,
            client_version: params.client_info.version,
            capabilities,
        };
        let response = self
            .protocol
            .handle(request)
            .map_err(protocol_failure_to_rpc)?;
        let ProtocolResponse::Handshake(handshake) = response else {
            return Err(JsonRpcError {
                code: -32603,
                message: "unexpected handshake response".into(),
                data: None,
            });
        };

        Ok(json!({
            "protocolVersion": params.protocol_version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "lore",
                "version": env!("CARGO_PKG_VERSION")
            },
            "_meta": {
                "loreProtocolVersion": handshake.protocol_version,
                "loreCapabilities": handshake.capabilities,
                "automationLevel": handshake.automation_level
            }
        }))
    }

    fn call_tool(&self, params: Value) -> std::result::Result<Value, JsonRpcError> {
        let params: ToolCallParams = serde_json::from_value(params)
            .map_err(|error| invalid_params(format!("invalid tools/call params: {error}")))?;
        let request = tool_request(&params.name, params.arguments)?;
        match self.protocol.handle(request) {
            Ok(response) => tool_success(response),
            Err(failure) => Ok(tool_failure(failure)),
        }
    }
}

fn tool_request(
    name: &str,
    arguments: Value,
) -> std::result::Result<ProtocolRequest, JsonRpcError> {
    let operation = match name {
        "lore_event_ingest" => "event.ingest",
        "lore_task_start" => "task.start",
        "lore_task_end" => "task.end",
        "lore_recall" => "recall",
        "lore_feedback" => "feedback",
        _ => return Err(invalid_params(format!("unknown Lore tool: {name}"))),
    };
    let mut object = match arguments {
        Value::Object(object) => object,
        _ => return Err(invalid_params("tool arguments must be an object")),
    };
    object.insert("operation".into(), Value::String(operation.into()));
    serde_json::from_value(Value::Object(object))
        .map_err(|error| invalid_params(format!("invalid arguments for {name}: {error}")))
}

fn tool_success(response: ProtocolResponse) -> std::result::Result<Value, JsonRpcError> {
    let structured = serde_json::to_value(response).map_err(internal_error)?;
    let text = serde_json::to_string_pretty(&structured).map_err(internal_error)?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": false
    }))
}

fn tool_failure(failure: ProtocolFailure) -> Value {
    let structured = serde_json::to_value(&failure).unwrap_or_else(|_| {
        json!({
            "code": "internal_error",
            "message": "could not serialize protocol failure"
        })
    });
    let text =
        serde_json::to_string_pretty(&structured).unwrap_or_else(|_| "protocol failure".into());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": true
    })
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "lore_event_ingest",
            "description": "Envia um EventEnvelope metadata-only ao Lore.",
            "inputSchema": {
                "type": "object",
                "properties": { "event": { "type": "object" } },
                "required": ["event"],
                "additionalProperties": false
            }
        },
        {
            "name": "lore_task_start",
            "description": "Inicia uma sessão, registra BeforeTask e retorna contexto não autoritativo quando metadata contém query, goal ou task.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "session_id": { "type": "string" },
                    "metadata": { "type": "object" }
                },
                "required": ["project_id"],
                "additionalProperties": false
            }
        },
        {
            "name": "lore_task_end",
            "description": "Finaliza uma sessão com resultado explícito.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "session_id": { "type": "string" },
                    "outcome": { "enum": ["success", "failed", "cancelled"] },
                    "metadata": { "type": "object" }
                },
                "required": ["project_id", "session_id", "outcome"],
                "additionalProperties": false
            }
        },
        {
            "name": "lore_recall",
            "description": "Busca Knowledge Units por texto, significado, escopo e metadados; retorna razões explicáveis.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "session_id": { "type": "string" },
                    "query": { "type": "string" },
                    "scope": { "enum": ["project", "global", "project_then_global"] },
                    "budget": { "type": "integer", "minimum": 1 },
                    "capabilities": { "type": "array", "items": { "type": "string" } },
                    "artifact": { "type": "string" },
                    "min_confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
                },
                "required": ["project_id", "query", "budget"],
                "additionalProperties": false
            }
        },
        {
            "name": "lore_feedback",
            "description": "Registra feedback append-only sobre uma Knowledge Unit para orientar reuso futuro.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "knowledge_id": { "type": "string" },
                    "version": { "type": "integer", "minimum": 1 },
                    "session_id": { "type": "string" },
                    "outcome": { "enum": ["used", "ignored", "corrected"] },
                    "note": { "type": "string" }
                },
                "required": ["project_id", "knowledge_id", "outcome"],
                "additionalProperties": false
            }
        }
    ])
}

fn serialize_response(response: JsonRpcResponse) -> Result<String> {
    Ok(serde_json::to_string(&response)?)
}

fn invalid_params(message: impl Into<String>) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: message.into(),
        data: None,
    }
}

fn invalid_params_with_data(message: impl Into<String>, data: Value) -> JsonRpcError {
    JsonRpcError {
        code: -32602,
        message: message.into(),
        data: Some(data),
    }
}

fn internal_error(error: serde_json::Error) -> JsonRpcError {
    JsonRpcError {
        code: -32603,
        message: "internal serialization error".into(),
        data: Some(json!({ "detail": error.to_string() })),
    }
}

fn protocol_failure_to_rpc(failure: ProtocolFailure) -> JsonRpcError {
    let data = serde_json::to_value(&failure).ok();
    JsonRpcError {
        code: -32602,
        message: failure.message,
        data,
    }
}

fn empty_object() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::{
        application::capture::{AppendOutcome, CaptureService, EventStore},
        domain::event::EventEnvelope,
    };

    #[derive(Default)]
    struct RecordingStore {
        event_ids: Mutex<HashSet<String>>,
    }

    impl EventStore for RecordingStore {
        fn append_event(&self, event: &EventEnvelope) -> crate::error::Result<AppendOutcome> {
            let mut event_ids = self.event_ids.lock().expect("recording store lock");
            Ok(if event_ids.insert(event.event_id.clone()) {
                AppendOutcome::Inserted
            } else {
                AppendOutcome::Duplicate
            })
        }

        fn pending_event_count(&self) -> crate::error::Result<u64> {
            Ok(self.event_ids.lock().expect("recording store lock").len() as u64)
        }
    }

    fn server() -> McpServer {
        let store = Arc::new(RecordingStore::default());
        McpServer::new(ProtocolService::new(Arc::new(CaptureService::new(store))))
    }

    #[test]
    fn initialize_returns_lore_capabilities_and_accepts_extra_fields() {
        let response = server()
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"Codex","version":"dev"},"capabilities":{},"extra":true}}"#,
            )
            .expect("initialize response")
            .expect("request response");
        let value: Value = serde_json::from_str(&response).expect("JSON response");
        assert_eq!(value["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(value["result"]["_meta"]["loreProtocolVersion"], 1);
        assert_eq!(
            value["result"]["_meta"]["loreCapabilities"][0],
            "event_ingest"
        );
    }

    #[test]
    fn initialize_accepts_codex_protocol_version() {
        let response = server()
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"Codex","version":"dev"},"capabilities":{}}}"#,
            )
            .expect("initialize response")
            .expect("request response");
        let value: Value = serde_json::from_str(&response).expect("JSON response");
        assert_eq!(value["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(value["result"]["serverInfo"]["name"], "lore");
    }

    #[test]
    fn initialize_rejects_unknown_mcp_version() {
        let response = server()
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"9.9","clientInfo":{"name":"Codex"},"capabilities":{}}}"#,
            )
            .expect("initialize response")
            .expect("request response");
        let value: Value = serde_json::from_str(&response).expect("JSON response");
        assert_eq!(value["error"]["code"], -32602);
        assert_eq!(
            value["error"]["data"]["code"],
            "unsupported_protocol_version"
        );
    }

    #[test]
    fn initialized_notification_has_no_response() {
        assert!(
            server()
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .expect("notification")
                .is_none()
        );
    }

    #[test]
    fn tools_call_uses_protocol_service_for_task_lifecycle() {
        let start = server()
            .handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lore_task_start","arguments":{"project_id":"p-1","session_id":"s-1","metadata":{"editor":"codex"}}}}"#,
            )
            .expect("task response")
            .expect("request response");
        let end = server()
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lore_task_end","arguments":{"project_id":"p-1","session_id":"s-1","outcome":"success"}}}"#,
            )
            .expect("task response")
            .expect("request response");
        let start: Value = serde_json::from_str(&start).expect("start JSON");
        let end: Value = serde_json::from_str(&end).expect("end JSON");
        assert_eq!(start["result"]["isError"], false);
        assert_eq!(end["result"]["isError"], false);
        assert_eq!(
            end["result"]["structuredContent"]["result"]["session_id"],
            "s-1"
        );
    }

    #[test]
    fn tool_call_degrades_recall_as_structured_error() {
        let response = server()
            .handle_line(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lore_recall","arguments":{"project_id":"p-1","query":"jwt","budget":5}}}"#,
            )
            .expect("recall response")
            .expect("request response");
        let value: Value = serde_json::from_str(&response).expect("JSON response");
        assert_eq!(value["result"]["isError"], true);
        assert_eq!(
            value["result"]["structuredContent"]["code"],
            "capability_unavailable"
        );
    }
}
