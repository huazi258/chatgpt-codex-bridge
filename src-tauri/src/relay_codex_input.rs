use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelayCodexInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Option<Vec<RelayCodexInputOption>>,
    #[serde(default)]
    pub is_other: bool,
    #[serde(default)]
    pub is_secret: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelayCodexInputOption { pub label: String, pub description: String }

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RelayCodexInputRequestParams {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    pub is_blocking: bool,
    #[serde(default)]
    pub auto_resolution_ms: Option<i64>,
    pub questions: Vec<RelayCodexInputQuestion>,
    #[serde(flatten)]
    pub compatibility: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelayCodexInputRequest {
    pub request_id: Value,
    pub params: RelayCodexInputRequestParams,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCodexInputSubmission { pub answers: Vec<(String, String)> }

pub fn parse_request(value: &Value) -> Result<RelayCodexInputRequest, String> {
    if value.get("method").and_then(Value::as_str) != Some("item/tool/requestUserInput") {
        return Err("不是 Codex 人工输入请求。".into());
    }
    let request_id = value.get("id").cloned().ok_or_else(|| "Codex 输入请求缺少 JSON-RPC id。".to_string())?;
    if !request_id.is_string() && !request_id.is_i64() && !request_id.is_u64() {
        return Err("Codex 输入请求的 JSON-RPC id 类型无效。".into());
    }
    let params: RelayCodexInputRequestParams = serde_json::from_value(value.get("params").cloned().ok_or_else(|| "Codex 输入请求缺少参数。".to_string())?)
        .map_err(|error| format!("Codex 输入请求格式无效：{error}"))?;
    if params.questions.is_empty() || params.questions.iter().any(|question| question.id.trim().is_empty()) {
        return Err("Codex 输入请求必须包含带 id 的问题。".into());
    }
    let mut ids = std::collections::HashSet::new();
    if params.questions.iter().any(|question| !ids.insert(&question.id)) {
        return Err("Codex 输入请求包含重复 question.id。".into());
    }
    Ok(RelayCodexInputRequest { request_id, params })
}

pub fn response_for(request_id: Value, submission: &RelayCodexInputSubmission) -> Value {
    let answers = submission.answers.iter().map(|(question_id, answer)| {
        (question_id.clone(), json!({ "answers": if answer.is_empty() { Vec::<String>::new() } else { vec![answer.clone()] } }))
    }).collect::<Map<_, _>>();
    json!({ "id": request_id, "result": { "answers": answers } })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_id_and_question_ids_are_preserved_for_response() {
        let request = parse_request(&json!({"id": 42, "method":"item/tool/requestUserInput", "params":{"threadId":"thread-a","turnId":"turn-a","itemId":"item-a","isBlocking":true,"autoResolutionMs":100,"questions":[{"id":"question-a","header":"A","question":"Q","options":null,"isOther":false,"isSecret":false}]}})).unwrap();
        let response = response_for(request.request_id, &RelayCodexInputSubmission { answers: vec![("question-a".into(), "".into())] });
        assert_eq!(response["id"], json!(42));
        assert_eq!(response["result"]["answers"]["question-a"]["answers"], json!([]));
    }
    #[test]
    fn rejects_duplicate_or_missing_question_identity() {
        let error = parse_request(&json!({"id":"x", "method":"item/tool/requestUserInput", "params":{"threadId":"t","turnId":"u","itemId":"i","isBlocking":true,"questions":[{"id":"","header":"","question":"","options":null}]}})).unwrap_err();
        assert!(error.contains("id") || error.contains("问题"), "{error}");
    }
}
