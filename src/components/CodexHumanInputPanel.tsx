import { useState } from 'react';
import type { RelayCodexInputAnswerInput, RelayCodexInputQuestion, RelayCodexInputRequest } from '../relay-codex-input';

export function CodexHumanInputPanel({ request, stopAfterTurn, onSubmit }: { request: RelayCodexInputRequest; stopAfterTurn: boolean; onSubmit: (answers: RelayCodexInputAnswerInput[]) => Promise<void> }) {
  const questions = JSON.parse(request.questionsJson) as RelayCodexInputQuestion[];
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const [submitting, setSubmitting] = useState(false);
  const actionable = request.status === 'PENDING';
  if (request.status === 'ANSWERED') return <section className="form-section codex-human-input"><h3>Codex 输入已确认</h3><p>Codex 已确认收到答案。</p></section>;
  if (!actionable) return <section className="form-section codex-human-input"><h3>Codex 输入请求</h3><p>{request.errorText || (request.status === 'ANSWERING' ? '答案已发送，正在等待 Codex 确认' : '该输入请求已不可提交。')}</p></section>;
  return <section className="form-section codex-human-input"><h3>Codex 需要你的输入</h3>{stopAfterTurn && <p>模块已请求终止；可完成当前 Codex 输入，回合结束后停止。</p>}{questions.map((question) => <label key={question.id}><strong>{question.header}</strong><span>{question.question}</span>{question.options?.length ? <small>参考选项：{question.options.map((option) => option.label).join('、')}</small> : null}{question.isSecret ? <input type="password" value={answers[question.id] ?? ''} onChange={(event) => setAnswers((current) => ({ ...current, [question.id]: event.target.value }))} /> : <textarea value={answers[question.id] ?? ''} onChange={(event) => setAnswers((current) => ({ ...current, [question.id]: event.target.value }))} />}</label>)}<button className="primary" disabled={submitting} onClick={() => { const outgoing=questions.map((question) => ({ questionId: question.id, answer: answers[question.id] ?? '' })); for (const question of questions.filter((question) => question.isSecret)) delete answers[question.id]; setSubmitting(true); void onSubmit(outgoing).finally(() => setSubmitting(false)); }}>提交给 Codex</button></section>;
}
