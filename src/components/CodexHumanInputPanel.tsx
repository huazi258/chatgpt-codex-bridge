import { useState } from 'react';
import type { RelayCodexInputAnswerInput, RelayCodexInputQuestion, RelayCodexInputRequest } from '../relay-codex-input';

function questionsFor(request: RelayCodexInputRequest): RelayCodexInputQuestion[] | null {
  try {
    const value: unknown = JSON.parse(request.questionsJson);
    return Array.isArray(value) ? value as RelayCodexInputQuestion[] : null;
  } catch {
    return null;
  }
}

export function CodexHumanInputPanel({ request, stopAfterTurn, onSubmit }: { request: RelayCodexInputRequest; stopAfterTurn: boolean; onSubmit: (answers: RelayCodexInputAnswerInput[]) => Promise<void> }) {
  const questions = questionsFor(request);
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  const metadata = <p><small>Cycle {request.cycleNumber ?? '—'} · Thread {request.codexThreadId} · Turn {request.codexTurnId} · 状态：{request.status}</small></p>;
  if (!questions) return <section className="form-section codex-human-input"><h3>Codex 输入请求</h3>{metadata}<p>输入请求数据无效，无法安全提交。请检查 Codex 运行状态。</p></section>;
  if (request.status === 'ANSWERED') return <section className="form-section codex-human-input"><h3>Codex 输入已确认</h3>{metadata}<p>Codex 已确认收到答案。</p></section>;
  if (request.status !== 'PENDING') {
    const message = request.errorText
      || (request.status === 'ANSWERING' ? '答案已发送，正在等待 Codex 确认'
        : request.status === 'EXPIRED' ? 'Codex App Server 已不再接受此输入请求。'
          : '输入请求因应用或运行时中断，请检查模块恢复状态。');
    return <section className="form-section codex-human-input"><h3>Codex 输入请求</h3>{metadata}<p>{message}</p></section>;
  }
  const safeQuestions = questions;
  function submit() {
    const outgoing = safeQuestions.map((question) => ({ questionId: question.id, answer: answers[question.id] ?? '' }));
    setAnswers((current) => Object.fromEntries(Object.entries(current).filter(([id]) => !safeQuestions.some((question) => question.id === id && question.isSecret))));
    setSubmitting(true);
    void onSubmit(outgoing).finally(() => setSubmitting(false));
  }
  return <section className="form-section codex-human-input"><h3>Codex 需要你的输入</h3>{metadata}{stopAfterTurn && <p>模块已请求终止；可完成当前 Codex 输入，回合结束后停止。</p>}{safeQuestions.map((question) => <label key={question.id}><strong>{question.header}</strong><span>{question.question}</span>{question.options?.length ? <small>参考选项：{question.options.map((option) => option.label).join('、')}</small> : null}{question.isSecret ? <input type="password" value={answers[question.id] ?? ''} onChange={(event) => setAnswers((current) => ({ ...current, [question.id]: event.target.value }))} /> : <textarea value={answers[question.id] ?? ''} onChange={(event) => setAnswers((current) => ({ ...current, [question.id]: event.target.value }))} />}</label>)}<button className="primary" disabled={submitting} onClick={submit}>提交给 Codex</button></section>;
}
