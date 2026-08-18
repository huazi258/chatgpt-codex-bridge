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
pub struct RelayCodexInputOption {
    pub label: String,
    pub description: String,
}

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
pub struct RelayCodexInputSubmission {
    pub answers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelayCodexResolvedRequest {
    pub request_id: Value,
    pub thread_id: String,
}

pub fn parse_request(value: &Value) -> Result<RelayCodexInputRequest, String> {
    if value.get("method").and_then(Value::as_str) != Some("item/tool/requestUserInput") {
        return Err("不是 Codex 人工输入请求。".into());
    }
    let request_id = value
        .get("id")
        .cloned()
        .ok_or_else(|| "Codex 输入请求缺少 JSON-RPC id。".to_string())?;
    if !request_id.is_string() && !request_id.is_i64() {
        return Err("Codex 输入请求的 JSON-RPC id 类型无效。".into());
    }
    let params: RelayCodexInputRequestParams = serde_json::from_value(
        value
            .get("params")
            .cloned()
            .ok_or_else(|| "Codex 输入请求缺少参数。".to_string())?,
    )
    .map_err(|error| format!("Codex 输入请求格式无效：{error}"))?;
    if params.thread_id.trim().is_empty()
        || params.turn_id.trim().is_empty()
        || params.item_id.trim().is_empty()
    {
        return Err("Codex 输入请求缺少 threadId、turnId 或 itemId。".into());
    }
    if params.questions.is_empty()
        || params
            .questions
            .iter()
            .any(|question| question.id.trim().is_empty())
    {
        return Err("Codex 输入请求必须包含带 id 的问题。".into());
    }
    let mut ids = std::collections::HashSet::new();
    if params
        .questions
        .iter()
        .any(|question| !ids.insert(&question.id))
    {
        return Err("Codex 输入请求包含重复 question.id。".into());
    }
    Ok(RelayCodexInputRequest { request_id, params })
}

pub fn parse_server_request_resolved(value: &Value) -> Result<RelayCodexResolvedRequest, String> {
    if value.get("method").and_then(Value::as_str) != Some("serverRequest/resolved") {
        return Err("不是 Codex request resolved 事件。".into());
    }
    let params = value
        .get("params")
        .ok_or_else(|| "resolved 事件缺少参数。".to_string())?;
    let request_id = params
        .get("requestId")
        .cloned()
        .ok_or_else(|| "resolved 事件缺少 requestId。".to_string())?;
    if !request_id.is_string() && !request_id.is_i64() {
        return Err("resolved requestId 类型无效。".into());
    }
    let thread_id = params
        .get("threadId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "resolved 事件缺少 threadId。".to_string())?
        .to_owned();
    Ok(RelayCodexResolvedRequest {
        request_id,
        thread_id,
    })
}

pub fn validate_submission(
    request: &RelayCodexInputRequest,
    submission: RelayCodexInputSubmission,
) -> Result<RelayCodexInputSubmission, String> {
    let expected: std::collections::HashSet<&str> = request
        .params
        .questions
        .iter()
        .map(|q| q.id.as_str())
        .collect();
    if submission.answers.len() != expected.len() {
        return Err("必须为每个 question.id 提供一项答案。".into());
    }
    let mut seen = std::collections::HashSet::new();
    for (id, _) in &submission.answers {
        if !expected.contains(id.as_str()) {
            return Err("答案包含未知 question.id。".into());
        }
        if !seen.insert(id.as_str()) {
            return Err("答案包含重复 question.id。".into());
        }
    }
    Ok(submission)
}

pub fn build_request_user_input_response(
    request: &RelayCodexInputRequest,
    submission: RelayCodexInputSubmission,
) -> Result<Value, String> {
    let submission = validate_submission(request, submission)?;
    let answers = submission.answers.iter().map(|(question_id, answer)| {
        (question_id.clone(), json!({ "answers": if answer.is_empty() { Vec::<String>::new() } else { vec![answer.clone()] } }))
    }).collect::<Map<_, _>>();
    Ok(json!({ "id": request.request_id, "result": { "answers": answers } }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_id_and_question_ids_are_preserved_for_response() {
        let request = parse_request(&json!({"id": 42, "method":"item/tool/requestUserInput", "params":{"threadId":"thread-a","turnId":"turn-a","itemId":"item-a","isBlocking":true,"autoResolutionMs":100,"questions":[{"id":"question-a","header":"A","question":"Q","options":null,"isOther":false,"isSecret":false}]}})).unwrap();
        let response = build_request_user_input_response(
            &request,
            RelayCodexInputSubmission {
                answers: vec![("question-a".into(), "".into())],
            },
        )
        .unwrap();
        assert_eq!(response["id"], json!(42));
        assert_eq!(
            response["result"]["answers"]["question-a"]["answers"],
            json!([])
        );
    }
    #[test]
    fn rejects_duplicate_or_missing_question_identity() {
        let error = parse_request(&json!({"id":"x", "method":"item/tool/requestUserInput", "params":{"threadId":"t","turnId":"u","itemId":"i","isBlocking":true,"questions":[{"id":"","header":"","question":"","options":null}]}})).unwrap_err();
        assert!(error.contains("id") || error.contains("问题"), "{error}");
    }
    #[test]
    fn resolved_preserves_string_and_signed_request_ids() {
        for id in [json!("request-a"), json!(-42), json!(42)] {
            assert_eq!(parse_server_request_resolved(&json!({"method":"serverRequest/resolved","params":{"requestId":id,"threadId":"thread"}})).unwrap().request_id, id);
        }
        assert!(parse_request(
            &json!({"id":9223372036854775808u64,"method":"item/tool/requestUserInput","params":{}})
        )
        .is_err());
    }
    #[test]
    fn submission_rejects_unknown_missing_and_duplicate_ids() {
        let request = parse_request(&json!({"id":"x","method":"item/tool/requestUserInput","params":{"threadId":"t","turnId":"u","itemId":"i","isBlocking":true,"questions":[{"id":"a","header":"","question":"","options":null},{"id":"b","header":"","question":"","options":null}]}})).unwrap();
        assert!(validate_submission(
            &request,
            RelayCodexInputSubmission {
                answers: vec![("a".into(), "free text".into()), ("a".into(), "".into())]
            }
        )
        .is_err());
        assert!(validate_submission(
            &request,
            RelayCodexInputSubmission {
                answers: vec![("a".into(), "".into()), ("x".into(), "".into())]
            }
        )
        .is_err());
    }

    #[test]
    fn response_preserves_every_question_id_without_turn_start() {
        let request = parse_request(&json!({"id":"request-a","method":"item/tool/requestUserInput","params":{"threadId":"thread","turnId":"turn","itemId":"item","isBlocking":true,"questions":[{"id":"a","header":"A","question":"A","options":[{"label":"reference","description":"only a reference"}]},{"id":"b","header":"B","question":"B","options":null}]}})).unwrap();
        let response = build_request_user_input_response(
            &request,
            RelayCodexInputSubmission {
                answers: vec![
                    ("a".into(), "free text outside options".into()),
                    ("b".into(), "".into()),
                ],
            },
        )
        .unwrap();
        assert_eq!(response["id"], "request-a");
        assert_eq!(
            response["result"]["answers"]["a"]["answers"],
            json!(["free text outside options"])
        );
        assert_eq!(response["result"]["answers"]["b"]["answers"], json!([]));
        assert!(response.get("method").is_none());
    }
}
