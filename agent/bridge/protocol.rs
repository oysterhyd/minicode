use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Bridge protocol spoken by this agent. Clients compare it during
/// `initialize` and ignore event types they do not know.
pub const PROTOCOL_VERSION: u64 = 2;
/// The oldest client protocol this agent still serves correctly.
pub const MIN_CLIENT_PROTOCOL: u64 = 2;
/// Optional features a client may rely on when listed here.
pub const CAPABILITIES: &[&str] = &[
    "workspaceTrust",
    "projectInstructionsChanged",
    "sessionPaging",
    "diagnosticEvents",
    "runIdentity",
    "asyncStart",
    "sessionSubscriptions",
    "fileCompletion",
    "messageQueue",
    "stdoutBackpressure",
];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RequestId {
    String(String),
    Int(i64),
}

impl RequestId {
    pub fn from_value(value: &Value) -> Result<Self> {
        if let Some(s) = value.as_str() {
            Ok(Self::String(s.to_string()))
        } else if let Some(n) = value.as_i64() {
            Ok(Self::Int(n))
        } else {
            bail!("request id must be a string or integer")
        }
    }

    pub fn to_json_value(&self) -> Value {
        match self {
            Self::String(s) => Value::String(s.clone()),
            Self::Int(n) => Value::from(*n),
        }
    }

    pub fn key_string(&self) -> String {
        self.to_json_value().to_string()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RpcRequest {
    pub id: Value,
    pub method: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}

fn empty_object() -> Value {
    json!({})
}

#[derive(Clone, Debug)]
pub struct ParsedRequest {
    pub id: RequestId,
    pub method: String,
    pub params: Value,
}

pub struct RpcResponse;

impl RpcResponse {
    pub fn ok(id: &RequestId, result: Value) -> Value {
        json!({"id": id.to_json_value(), "result": result})
    }

    pub fn err(id: &RequestId, error: &str) -> Value {
        json!({"id": id.to_json_value(), "error": error})
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Control,
    Network,
    Router,
}

/// Classify a bridge RPC method into its execution lane and slot reservation policy.
pub fn classify_method(method: &str) -> Option<MethodKind> {
    match method {
        "cancelTurn" | "resolveApproval" => Some(MethodKind::Control),
        "fetchServiceModels" | "testServiceConnection" | "refreshMcp" => Some(MethodKind::Network),
        "initialize"
        | "getConfiguration"
        | "getCredentialImportInfo"
        | "importCommandCode"
        | "listModels"
        | "saveService"
        | "deleteService"
        | "setDefaultModel"
        | "saveAgent"
        | "setAgentEnabled"
        | "deleteAgent"
        | "getInstructions"
        | "saveInstructions"
        | "listSessions"
        | "listTasks"
        | "getSession"
        | "getSessionPage"
        | "renameSession"
        | "pinSession"
        | "deleteSession"
        | "listFiles"
        | "readArtifact"
        | "selectSession"
        | "getState"
        | "resetSession"
        | "setModel"
        | "setTuiProvider"
        | "setEffort"
        | "setPermissionMode"
        | "setBudget"
        | "setAcceptance"
        | "clearAlwaysAllow"
        | "getCapabilities"
        | "getLocalExtensions"
        | "setLocalExtensionEnabled"
        | "getWorkspaceTrust"
        | "trustWorkspace"
        | "setPluginEnabled"
        | "lockPlugins"
        | "setSkillActive"
        | "importAttachments"
        | "attachmentPath"
        | "sendPrompt"
        | "enqueuePrompt"
        | "listQueue"
        | "deleteQueuedPrompt"
        | "resumeQueue"
        | "runSlash" => Some(MethodKind::Router),
        _ => None,
    }
}

pub fn parse_request_line(line: &[u8], over_limit: bool) -> Result<ParsedRequest> {
    if over_limit {
        bail!("request exceeds the size limit");
    }
    let value: Value = serde_json::from_slice(line)?;
    if !value.is_object() {
        bail!("request must be an object");
    }
    let id = RequestId::from_value(value.get("id").unwrap_or(&Value::Null))?;
    let request: RpcRequest = serde_json::from_value(value)
        .map_err(|_| anyhow::anyhow!("method must be a string and params must be an object"))?;
    if !request.params.is_object() {
        bail!("method must be a string and params must be an object");
    }
    Ok(ParsedRequest {
        id,
        method: request.method,
        params: request.params,
    })
}

pub fn extract_valid_id(line: &[u8]) -> Option<Value> {
    serde_json::from_slice::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())
        .filter(|v| v.is_string() || v.is_i64())
}

// Recover only a top-level id from the retained prefix. JSON parsing handles
// escaped strings and skips nested ids; an id beyond the prefix is unknown.
pub(super) fn request_id_prefix(prefix: &[u8]) -> Option<Value> {
    struct FindId<'a>(&'a mut Option<Value>);
    impl<'de> serde::de::Visitor<'de> for FindId<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("request object")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> std::result::Result<(), M::Error> {
            while let Some(key) = map.next_key::<String>()? {
                if key == "id" {
                    let id = map.next_value::<Value>()?;
                    if id.is_string() || id.is_i64() {
                        *self.0 = Some(id);
                    }
                    return Ok(());
                }
                map.next_value::<serde::de::IgnoredAny>()?;
            }
            Ok(())
        }
    }
    let mut id = None;
    // A partial map is expected: keep a fully parsed id even if the final
    // map delimiter or the rest of the oversized body is absent.
    let _ = serde::Deserializer::deserialize_map(
        &mut serde_json::Deserializer::from_slice(prefix),
        FindId(&mut id),
    );
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{Router, output::is_transient_stream_event};
    use crate::store::Store;
    use std::sync::Arc;

    #[test]
    fn oversized_prefix_ids_are_top_level_and_json_decoded() {
        for (prefix, expected) in [
            (r#"{"id":42,"params":{"text":"partial"#, Some(json!(42))),
            (r#"{"id":"a\u4e2d\"b","params":{"#, Some(json!("a中\"b"))),
            (
                r#"{"params":{"id":8},"id":-2,"text":"partial"#,
                Some(json!(-2)),
            ),
            (r#"{"params":{"id":8,"text":"partial"#, None),
            (r#"{"id":[],"text":"partial"#, None),
        ] {
            assert_eq!(request_id_prefix(prefix.as_bytes()), expected, "{prefix}");
        }
    }

    #[test]
    fn parse_request_line_enforces_id_method_params_and_size_limit() {
        let req = parse_request_line(br#"{"id":"req-1","method":"getState"}"#, false).unwrap();
        assert_eq!(req.id, RequestId::String("req-1".into()));
        assert_eq!(req.method, "getState");
        assert_eq!(req.params, json!({}));

        let int_req = parse_request_line(
            br#"{"id":7,"method":"setEffort","params":{"effort":"high"}}"#,
            false,
        )
        .unwrap();
        assert_eq!(int_req.id, RequestId::Int(7));
        assert_eq!(int_req.params["effort"], "high");

        assert!(parse_request_line(br#"{"id":"1","method":"getState"}"#, true).is_err());
        assert!(parse_request_line(br#"{"id":1.5,"method":"getState"}"#, false).is_err());
        assert!(parse_request_line(br#"{"id":1,"method":"getState","params":[]}"#, false).is_err());
        assert!(parse_request_line(br#"[]"#, false).is_err());
    }

    #[tokio::test]
    async fn manifest_matches_runtime_constants_methods_events_and_state_snapshot() {
        let manifest: Value =
            serde_json::from_str(include_str!("../assets/bridge-protocol.v2.json")).unwrap();
        assert_eq!(manifest["protocolVersion"].as_u64(), Some(PROTOCOL_VERSION));
        assert_eq!(
            manifest["minClientProtocol"].as_u64(),
            Some(MIN_CLIENT_PROTOCOL)
        );
        let caps = manifest["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(caps, CAPABILITIES);

        for (bucket, expected) in [
            ("control", MethodKind::Control),
            ("network", MethodKind::Network),
            ("router", MethodKind::Router),
        ] {
            for method in manifest["methods"][bucket].as_array().unwrap() {
                let name = method.as_str().unwrap();
                assert_eq!(classify_method(name), Some(expected), "method {name}");
            }
        }
        for event in manifest["events"]["transient"].as_array().unwrap() {
            assert!(
                is_transient_stream_event(&json!({"event": event})),
                "expected transient event: {event}"
            );
        }
        for event in manifest["events"]["critical"].as_array().unwrap() {
            assert!(
                !is_transient_stream_event(&json!({"event": event})),
                "expected critical event: {event}"
            );
        }

        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new(Store::open(":memory:").unwrap(), Arc::new(|_| {}));
        router.configuration = crate::configuration::Configuration {
            path: dir.path().join("config.json"),
        };
        let state = router.handle("getState", &json!({})).await.unwrap();
        for field in manifest["stateFields"].as_array().unwrap() {
            let key = field.as_str().unwrap();
            assert!(
                state.as_object().unwrap().contains_key(key),
                "getState missing manifest field {key}"
            );
        }
        assert!(
            router
                .handle("unknownContractMethod", &json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("未知请求")
        );
    }
}
