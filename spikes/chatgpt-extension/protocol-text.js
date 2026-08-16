(() => {
const protocolReplyTimeoutMs = 90_000;
const incompleteProtocolGraceMs = 90_000;

function protocolReplyDeadlineMs(requireProtocolJson, hasPendingProtocolCandidate, timeoutMs = protocolReplyTimeoutMs) {
  return timeoutMs + (requireProtocolJson && hasPendingProtocolCandidate ? incompleteProtocolGraceMs : 0);
}

function findProtocolReplyOutsideBaseline(replies, baselineProtocolJson) {
  return replies.find((reply) => (
    Boolean(reply.protocolJson) && !baselineProtocolJson.has(reply.protocolJson)
  )) ?? null;
}

function protocolJsonObjects(text) {
  const matches = [];
  for (let start = text.indexOf('{'); start >= 0; start = text.indexOf('{', start + 1)) {
    let depth = 0;
    let inString = false;
    let escaped = false;
    for (let end = start; end < text.length; end += 1) {
      const character = text[end];
      if (inString) {
        if (escaped) escaped = false;
        else if (character === '\\') escaped = true;
        else if (character === '"') inString = false;
        continue;
      }
      if (character === '"') {
        inString = true;
        continue;
      }
      if (character === '{') depth += 1;
      if (character === '}') depth -= 1;
      if (depth !== 0) continue;

      const json = text.slice(start, end + 1);
      try {
        const parsed = JSON.parse(json);
        if (parsed && typeof parsed === 'object' && typeof parsed.state === 'string') {
          matches.push({ json, start, end: end + 1 });
        }
      } catch {
        // This opening brace did not delimit a protocol JSON object.
      }
      break;
    }
  }
  return matches;
}

function extractProtocolJsonObject(visibleText, codeBlocks) {
  const codeJson = [...new Set(codeBlocks.flatMap((block) => protocolJsonObjects(block.trim()).map((match) => match.json)))];
  if (codeJson.length === 1) return codeJson[0];
  if (codeJson.length > 1) return null;

  const visibleJson = protocolJsonObjects(visibleText.trim());
  return visibleJson.length === 1 ? visibleJson[0].json : null;
}

function restoreProtocolCodeBlock(visibleText, codeBlocks) {
  const plainText = visibleText.trim();
  if (plainText.includes('```json')) return plainText;
  const visibleJson = protocolJsonObjects(plainText);
  const json = extractProtocolJsonObject(plainText, codeBlocks);
  if (!json) return plainText;

  const visibleMatch = visibleJson.find((match) => match.json === json);
  const naturalLanguage = visibleMatch ? plainText.slice(0, visibleMatch.start).trimEnd() : plainText;
  return `${naturalLanguage ? `${naturalLanguage}\n\n` : ''}\`\`\`json\n${json}\n\`\`\``;
}

globalThis.restoreProtocolCodeBlock = restoreProtocolCodeBlock;
globalThis.extractProtocolJsonObject = extractProtocolJsonObject;
globalThis.protocolReplyDeadlineMs = protocolReplyDeadlineMs;
globalThis.findProtocolReplyOutsideBaseline = findProtocolReplyOutsideBaseline;
globalThis.chatGptMiddlewareProtocolTextVersion = '1.2.0';
})();
