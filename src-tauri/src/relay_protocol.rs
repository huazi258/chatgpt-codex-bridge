#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlBlock {
    CodexPrompt(String),
    ModuleDone,
    Blocked(String),
    CodexInput(Vec<String>),
}

pub fn parse_terminal_control_block(
    reply: &str,
    input_question_count: Option<usize>,
) -> Result<ControlBlock, String> {
    let text = reply.trim_end();
    let mut found = Vec::new();
    if let Some(body) =
        parse_terminal_wrapped(text, "@@@CODEX_PROMPT@@@", "@@@END_CODEX_PROMPT@@@")?
    {
        found.push(ControlBlock::CodexPrompt(body));
    }
    if let Some(body) = parse_terminal_wrapped(text, "@@@BLOCKED@@@", "@@@END_BLOCKED@@@")? {
        found.push(ControlBlock::Blocked(body));
    }
    if let Some(body) = parse_terminal_wrapped(text, "@@@CODEX_INPUT@@@", "@@@END_CODEX_INPUT@@@")?
    {
        let answers = parse_numbered_answers(&body)?;
        if let Some(expected) = input_question_count {
            if answers.len() != expected {
                return Err(format!(
                    "CODEX_INPUT 需要 {expected} 项答案，实际为 {} 项",
                    answers.len()
                ));
            }
        } else {
            return Err("当前没有待回答的 Codex 输入请求".into());
        }
        found.push(ControlBlock::CodexInput(answers));
    }
    if text.ends_with("@@@MODULE_DONE@@@") {
        let prefix = text.trim_end_matches("@@@MODULE_DONE@@@");
        if !prefix.contains("@@@") {
            found.push(ControlBlock::ModuleDone);
        }
    }
    if found.len() != 1 {
        return Err("自动化回复末尾必须且只能包含一个有效控制块".into());
    }
    Ok(found.into_iter().next().expect("one control block"))
}

fn parse_terminal_wrapped(text: &str, start: &str, end: &str) -> Result<Option<String>, String> {
    if !text.ends_with(end) {
        return Ok(None);
    }
    let prefix = &text[..text.len() - end.len()];
    let Some(offset) = prefix.rfind(start) else {
        return Ok(None);
    };
    if prefix[..offset].contains("@@@") {
        return Err("自动化回复中出现了多个或不完整的控制块".into());
    }
    let body = &prefix[offset + start.len()..];
    let body = body
        .strip_prefix("\r\n")
        .or_else(|| body.strip_prefix('\n'))
        .unwrap_or(body);
    let body = body
        .strip_suffix("\r\n")
        .or_else(|| body.strip_suffix('\n'))
        .unwrap_or(body);
    if body.trim().is_empty() {
        return Err("控制块内容不能为空".into());
    }
    Ok(Some(body.to_string()))
}

fn parse_numbered_answers(body: &str) -> Result<Vec<String>, String> {
    let mut answers = Vec::new();
    for (index, line) in body
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let expected = format!("{}.", index + 1);
        let value = line
            .trim()
            .strip_prefix(&expected)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("CODEX_INPUT 第 {} 项必须以 `{expected}` 开头", index + 1))?;
        answers.push(value.to_string());
    }
    if answers.is_empty() {
        return Err("CODEX_INPUT 不能为空".into());
    }
    Ok(answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_prompt_keeps_body_verbatim() {
        assert_eq!(
            parse_terminal_control_block(
                "说明\n@@@CODEX_PROMPT@@@\n做事\n@@@END_CODEX_PROMPT@@@",
                None
            ),
            Ok(ControlBlock::CodexPrompt("做事".into()))
        );
    }
    #[test]
    fn prompt_preserves_intentional_leading_and_trailing_spaces() {
        assert_eq!(
            parse_terminal_control_block(
                "@@@CODEX_PROMPT@@@\n  保留这两个空格  \n@@@END_CODEX_PROMPT@@@",
                None
            ),
            Ok(ControlBlock::CodexPrompt("  保留这两个空格  ".into()))
        );
    }
    #[test]
    fn manual_like_text_is_not_a_control_block() {
        assert!(parse_terminal_control_block("引用 @@@MODULE_DONE@@@ 讨论", None).is_err());
    }
    #[test]
    fn input_requires_the_pending_question_count() {
        assert_eq!(
            parse_terminal_control_block(
                "@@@CODEX_INPUT@@@\n1. 是\n2. 否\n@@@END_CODEX_INPUT@@@",
                Some(2)
            ),
            Ok(ControlBlock::CodexInput(vec!["是".into(), "否".into()]))
        );
        assert!(parse_terminal_control_block(
            "@@@CODEX_INPUT@@@\n1. 是\n@@@END_CODEX_INPUT@@@",
            Some(2)
        )
        .is_err());
    }
    #[test]
    fn rejects_multiple_terminal_blocks() {
        assert!(parse_terminal_control_block("@@@CODEX_PROMPT@@@\n甲\n@@@END_CODEX_PROMPT@@@\n@@@CODEX_PROMPT@@@\n乙\n@@@END_CODEX_PROMPT@@@", None).is_err());
    }
    #[test]
    fn module_done_can_follow_explanation() {
        assert_eq!(
            parse_terminal_control_block("已完成。\n@@@MODULE_DONE@@@", None),
            Ok(ControlBlock::ModuleDone)
        );
    }
}
