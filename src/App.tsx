import { useEffect, useLayoutEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  Check,
  ChevronDown,
  ChevronRight,
  Clock3,
  Copy,
  Languages,
  LogIn,
  LogOut,
  MessageCircleQuestion,
  RefreshCw,
  Send,
  Settings,
  Sparkles,
  Trash2,
  UserRound,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import "./App.css";
import glossLogo from "./assets/gloss-logo.png";
import { remarkCjkStrongBoundaries } from "./markdown";

type Action = "triage" | "translate";
type Mode = "toolbar" | "card" | "history" | "settings" | "profile" | "signin";
type Theme = "system" | "light" | "dark";
type CefrLevel = "A1" | "A2" | "B1" | "B2" | "C1" | "C2" | "insufficient_evidence";
type ProfileDimensionName = "reading" | "vocabulary" | "grammar" | "pragmatics";
type Selection = { text: string; anchor?: { left: number; top: number; width: number; height: number } };
type OverlayState = { status: "idle" } | { status: "ready"; selection: Selection } | { status: "capture_error"; message: string };
type Message = { id: string; turnId: string; role: "user" | "assistant"; content: string; timestamp: string; intent?: "explain_selection"; attemptId?: string };
type Session = { sessionId: string; action: Action; selectedText: string; messages: Message[] };
type SessionSummary = { sessionId: string; action: Action; preview: string; updatedAt: string };
type SettingsValue = { shortcut: string; theme: Theme; proxyUrl: string };
type SettingsSaveState = { status: "idle" | "saving" | "saved" } | { status: "error"; message: string };
type AuthStatus = { status: "signed_out" } | { status: "connected"; expiresAtMs: number } | { status: "error"; message: string };
type ProfileEstimate = { level: CefrLevel; confidence: number; rationale: string };
type ProfileDimension = { dimension: ProfileDimensionName; level: CefrLevel; confidence: number; evidence: string; updatedAt: string };
type ProfileObservation = { dimension: ProfileDimensionName; cefrLevel: CefrLevel; descriptor: string; evidence: string; evidenceCount: number; firstSeenAt: string; lastSeenAt: string };
type LearnerProfile = { framework: "CEFR"; updatedAt: string; conversationsObserved: number; overall: ProfileEstimate; dimensions: ProfileDimension[]; recentObservations: ProfileObservation[] };
type AgentEvent =
  | { type: "started"; sessionId: string; attemptId: string }
  | { type: "delta"; sessionId: string; attemptId: string; delta: string }
  | { type: "completed"; sessionId: string; attemptId: string; session: Session }
  | { type: "failed"; sessionId: string; attemptId: string; message: string; partial: string; incomplete: boolean };

const DEFAULT_SETTINGS: SettingsValue = { shortcut: "ctrl+alt+shift+t", theme: "system", proxyUrl: "" };
const isTauri = "__TAURI_INTERNALS__" in window;
const DEMO_SESSION: Session = {
  sessionId: "preview",
  action: "triage",
  selectedText: "pleased to announce ive been awarded the title of most obscure and forgotten former fyad",
  messages: [
    { id: "u", turnId: "t", role: "user", content: "pleased to announce ive been awarded the title of most obscure and forgotten former fyad", timestamp: new Date().toISOString() },
    { id: "a", turnId: "t", role: "assistant", content: "## Quick read\n\n很高兴宣布，我被授予了“最默默无闻、最被遗忘的前 FYAD 成员”这一称号。语气带有明显的自嘲和网络幽默。\n\n## Key points\n\n### ive\n\n`ive` 是聊天中的非正式拼写，标准写法是 `I've`。\n\n### been awarded\n\n这是现在完成时的被动语态：`have been + past participle`，强调已经发生且与现在相关的结果。\n\n### mock award\n\n把负面评价包装成正式奖项，是一种夸张的自嘲。", timestamp: new Date().toISOString() },
  ],
};
const DEMO_PROFILE: LearnerProfile = {
  framework: "CEFR",
  updatedAt: new Date().toISOString(),
  conversationsObserved: 4,
  overall: { level: "B1", confidence: 0.64, rationale: "能够理解真实技术文本的主旨；遇到习惯表达和隐含语气时，适合补充针对性说明。" },
  dimensions: [
    { dimension: "reading", level: "B1", confidence: 0.72, evidence: "能够跟随主要论点，并识别作者给出的实际建议。", updatedAt: new Date().toISOString() },
    { dimension: "vocabulary", level: "B1", confidence: 0.58, evidence: "能够理解常见技术词汇，并针对陌生表达提出具体问题。", updatedAt: new Date().toISOString() },
    { dimension: "grammar", level: "B1", confidence: 0.61, evidence: "能够识别常见从句结构；理解压缩句式时偶尔需要提示。", updatedAt: new Date().toISOString() },
    { dimension: "pragmatics", level: "A2", confidence: 0.46, evidence: "理解隐含语气、反讽和对话意图时仍会受益于解释。", updatedAt: new Date().toISOString() },
  ],
  recentObservations: [
    { dimension: "pragmatics", cefrLevel: "A2", descriptor: "正在建立对短帖中隐含语气的识别能力。", evidence: "曾追问一段产品公告是在陈述事实还是表达调侃。", evidenceCount: 2, firstSeenAt: new Date().toISOString(), lastSeenAt: new Date().toISOString() },
  ],
};

function App() {
  const [mode, setMode] = useState<Mode>(isTauri ? "toolbar" : "card");
  const [selection, setSelection] = useState<Selection | null>(isTauri ? null : { text: DEMO_SESSION.selectedText });
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [session, setSession] = useState<Session | null>(isTauri ? null : DEMO_SESSION);
  const [streamText, setStreamText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [auth, setAuth] = useState<AuthStatus>({ status: "signed_out" });
  const [pendingAction, setPendingAction] = useState<Action | null>(null);
  const [history, setHistory] = useState<SessionSummary[]>([]);
  const [settings, setSettings] = useState<SettingsValue>(DEFAULT_SETTINGS);
  const [draftSettings, setDraftSettings] = useState<SettingsValue>(DEFAULT_SETTINGS);
  const [settingsSaveState, setSettingsSaveState] = useState<SettingsSaveState>({ status: "idle" });
  const [profile, setProfile] = useState<LearnerProfile | null>(isTauri ? null : DEMO_PROFILE);
  const [profileLoading, setProfileLoading] = useState(false);
  const [profileError, setProfileError] = useState<string | null>(null);
  const [question, setQuestion] = useState("");
  const [copied, setCopied] = useState(false);
  const [notice, setNotice] = useState("");
  const settingsSaveGeneration = useRef(0);

  const visibleMessages = useMemo(() => session?.messages.filter((_, index) => index > 0) ?? [], [session]);
  const lastAnswer = [...visibleMessages].reverse().find((message) => message.role === "assistant")?.content;

  useEffect(() => {
    if (!isTauri) return;
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;
    Promise.all([
      listen<OverlayState>("overlay-state", ({ payload }) => {
        if (payload.status === "ready") {
          setSelection(payload.selection);
          setCaptureError(null);
          setSession(null);
          setStreamText("");
          setError(null);
          setMode("toolbar");
        } else if (payload.status === "capture_error") {
          setSelection(null);
          setCaptureError(payload.message);
          setMode("toolbar");
        }
      }),
      listen<AgentEvent>("agent-event", ({ payload }) => {
        if (payload.type === "started") {
          setBusy(true);
          setError(null);
          setStreamText("");
        } else if (payload.type === "delta") {
          setStreamText((current) => current + payload.delta);
        } else if (payload.type === "completed") {
          setSession(payload.session);
          setStreamText("");
          setBusy(false);
        } else if (payload.type === "failed") {
          setStreamText(payload.partial);
          setBusy(false);
          setError(payload.message);
        }
      }),
      listen<AuthStatus>("auth-status", ({ payload }) => {
        setAuth(payload);
        if (payload.status === "connected") {
          setError(null);
          setNotice("Connected to ChatGPT");
          setMode("toolbar");
          invoke("collapse_overlay").catch(() => undefined);
        } else if (payload.status === "error") {
          setNotice("");
          setError(payload.message);
        }
      }),
      listen("show-settings", () => {
        invoke<SettingsValue>("get_settings").then((value) => {
          setSettings(value);
          setDraftSettings(value);
        }).catch(() => undefined);
        setSettingsSaveState({ status: "idle" });
        setError(null);
        setMode("settings");
      }),
    ]).then((values) => cancelled ? values.forEach((unlisten) => unlisten()) : unlisteners.push(...values));
    invoke<OverlayState>("get_overlay_state").then((state) => {
      if (state.status === "ready") setSelection(state.selection);
      else if (state.status === "capture_error") setCaptureError(state.message);
    }).catch(() => undefined);
    invoke<AuthStatus>("get_auth_status").then(setAuth).catch(() => undefined);
    invoke<SettingsValue>("get_settings").then((value) => { setSettings(value); setDraftSettings(value); }).catch(() => undefined);
    return () => { cancelled = true; unlisteners.forEach((unlisten) => unlisten()); };
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = mode === "settings" ? draftSettings.theme : settings.theme;
  }, [draftSettings.theme, mode, settings.theme]);
  useEffect(() => {
    const generation = ++settingsSaveGeneration.current;
    if (settingsEqual(draftSettings, settings)) return;
    const requested = { ...draftSettings };
    const delay = requested.proxyUrl !== settings.proxyUrl ? 450 : 0;
    const timer = window.setTimeout(() => {
      const proxyError = validateProxy(requested.proxyUrl);
      if (proxyError) {
        if (generation === settingsSaveGeneration.current) setSettingsSaveState({ status: "error", message: proxyError });
        return;
      }
      setSettingsSaveState({ status: "saving" });
      if (!isTauri) {
        setSettings(requested);
        setSettingsSaveState({ status: "saved" });
        return;
      }
      invoke<SettingsValue>("update_settings", { settings: requested }).then((saved) => {
        if (generation !== settingsSaveGeneration.current) return;
        setSettings(saved);
        setDraftSettings((current) => settingsEqual(current, requested) ? saved : current);
        setSettingsSaveState({ status: "saved" });
      }).catch((cause) => {
        if (generation === settingsSaveGeneration.current) setSettingsSaveState({ status: "error", message: errorMessage(cause) });
      });
    }, delay);
    return () => window.clearTimeout(timer);
  }, [draftSettings, settings]);
  useEffect(() => {
    const preventBrowserMenu = (event: globalThis.MouseEvent) => event.preventDefault();
    window.addEventListener("contextmenu", preventBrowserMenu);
    return () => window.removeEventListener("contextmenu", preventBrowserMenu);
  }, []);
  useEffect(() => {
    const escape = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  });

  async function runAction(action: Action) {
    setPendingAction(action);
    setError(null);
    if (auth.status !== "connected") {
      setMode("signin");
      if (isTauri) await invoke("expand_overlay").catch(() => undefined);
      return;
    }
    setMode("card");
    setBusy(true);
    try { setSession(await invoke<Session>("start_action", { action })); }
    catch (cause) { setBusy(false); setError(errorMessage(cause)); }
  }

  async function signIn() {
    setError(null);
    try {
      const result = await invoke<{ authorizationUrl: string }>("start_oauth_login");
      await openUrl(result.authorizationUrl);
      setNotice("Finish signing in in your browser");
    } catch (cause) { setError(errorMessage(cause)); }
  }

  async function signOut() {
    try { await invoke("logout_oauth"); setAuth({ status: "signed_out" }); setNotice("Signed out"); }
    catch (cause) { setError(errorMessage(cause)); }
  }

  async function submitQuestion(event: FormEvent) {
    event.preventDefault();
    const content = question.trim();
    if (!content || busy) return;
    setQuestion(""); setBusy(true); setError(null);
    try { setSession(await invoke<Session>("submit_follow_up", { content })); }
    catch (cause) { setBusy(false); setError(errorMessage(cause)); }
  }

  async function explainSelection(selectedText: string) {
    if (busy) return;
    setBusy(true); setError(null);
    try { setSession(await invoke<Session>("explain_selection", { selectedText })); }
    catch (cause) { setBusy(false); setError(errorMessage(cause)); }
  }

  async function retry() {
    setBusy(true); setError(null); setStreamText("");
    try { await invoke("retry_turn"); }
    catch (cause) { setBusy(false); setError(errorMessage(cause)); }
  }

  async function showHistory() {
    setMode("history"); setError(null);
    if (isTauri) await invoke("expand_overlay").catch((cause) => setError(errorMessage(cause)));
    try { setHistory(await invoke<SessionSummary[]>("list_history")); }
    catch (cause) { setError(errorMessage(cause)); }
  }

  async function openHistory(sessionId: string) {
    try {
      const value = await invoke<Session>("load_history", { sessionId });
      setSession(value); setSelection({ text: value.selectedText }); setMode("card");
    } catch (cause) { setError(errorMessage(cause)); }
  }

  async function removeHistory(event: MouseEvent, sessionId: string) {
    event.stopPropagation();
    try { await invoke("delete_history", { sessionId }); setHistory((items) => items.filter((item) => item.sessionId !== sessionId)); }
    catch (cause) { setError(errorMessage(cause)); }
  }

  function showSettings() { setDraftSettings(settings); setSettingsSaveState({ status: "idle" }); setError(null); setMode("settings"); }
  async function showSettingsFromToolbar() {
    setDraftSettings(settings);
    setSettingsSaveState({ status: "idle" });
    setError(null);
    setMode("settings");
    if (isTauri) await invoke("show_settings").catch((cause) => setError(errorMessage(cause)));
  }

  async function showProfile() {
    setMode("profile");
    setProfileError(null);
    if (!isTauri) {
      setProfile(DEMO_PROFILE);
      return;
    }
    setProfileLoading(true);
    try { setProfile(await invoke<LearnerProfile>("get_learner_profile")); }
    catch (cause) { setProfileError(errorMessage(cause)); }
    finally { setProfileLoading(false); }
  }

  async function returnToToolbar() {
    setMode("toolbar");
    if (isTauri) await invoke("collapse_overlay").catch((cause) => setError(errorMessage(cause)));
  }

  async function copyAnswer() {
    const text = streamText || lastAnswer;
    if (!text) return;
    await navigator.clipboard.writeText(text);
    setCopied(true); window.setTimeout(() => setCopied(false), 1400);
  }
  function close() { if (isTauri) invoke("hide_overlay").catch(() => undefined); }

  return (
    <main className={`shell ${mode === "toolbar" ? "is-toolbar" : "is-card"}`} aria-label="Gloss">
      {mode === "toolbar" ? (
        <Toolbar selection={selection} error={captureError} onAction={runAction} onHistory={showHistory} onSettings={showSettingsFromToolbar} onClose={close} />
      ) : (
        <section className={`card mode-${mode}`} aria-label="Gloss reading companion">
          <CardHeader mode={mode} action={session?.action ?? pendingAction} onBack={returnToToolbar} onHistory={showHistory} onSettings={showSettings} onClose={close} />
          {mode === "signin" && <SignInPanel onSignIn={signIn} error={error} notice={notice} />}
          {mode === "history" && <HistoryPanel items={history} error={error} onOpen={openHistory} onDelete={removeHistory} />}
          {mode === "settings" && <SettingsPanel value={draftSettings} auth={auth} error={error} saveState={settingsSaveState} onChange={setDraftSettings} onProfile={showProfile} onSignIn={signIn} onSignOut={signOut} />}
          {mode === "profile" && <ProfilePanel value={profile} loading={profileLoading} error={profileError} />}
          {mode === "card" && <ResultPanel session={session} selection={selection} streamText={streamText} busy={busy} error={error} question={question} copied={copied} onQuestionChange={setQuestion} onQuestionSubmit={submitQuestion} onExplainSelection={explainSelection} onRetry={retry} onCopy={copyAnswer} />}
          <div className="sr-status" role="status" aria-live="polite">{notice || (busy ? "Gloss is thinking" : error ?? "")}</div>
        </section>
      )}
    </main>
  );
}

function Toolbar({ selection, error, onAction, onHistory, onSettings, onClose }: { selection: Selection | null; error: string | null; onAction: (action: Action) => void; onHistory: () => void; onSettings: () => void; onClose: () => void }) {
  function startToolbarDrag(event: MouseEvent<HTMLElement>) {
    if (!isTauri || event.button !== 0) return;
    const target = event.target as Element;
    if (target.closest("button, input, a, [role='button']")) return;
    event.preventDefault();
    getCurrentWindow().startDragging().catch(() => undefined);
  }

  return (
    <section className="toolbar" aria-label="Text actions" onMouseDown={startToolbarDrag}>
      <button className="toolbar-mark" aria-label="Open history" onClick={onHistory}><img src={glossLogo} alt="" /></button>
      <p className={error ? "toolbar-error" : "selection-peek"}>{error ? "No readable selection" : selection?.text || "Selected text"}</p>
      <div className="toolbar-actions">
        <button className="action-button primary" onClick={() => onAction("triage")} disabled={!selection}><MessageCircleQuestion size={16} /><span>Triage</span></button>
        <button className="action-button" onClick={() => onAction("translate")} disabled={!selection}><Languages size={16} /><span>Translate</span></button>
      </div>
      <button className="icon-button compact" aria-label="Open settings" onClick={onSettings}><Settings size={15} /></button>
      <button className="icon-button compact" aria-label="Close Gloss" onClick={onClose}><X size={15} /></button>
    </section>
  );
}

function CardHeader({ mode, action, onBack, onHistory, onSettings, onClose }: { mode: Mode; action: Action | null; onBack: () => void; onHistory: () => void; onSettings: () => void; onClose: () => void }) {
  const title = mode === "history" ? "History" : mode === "settings" ? "Settings" : mode === "profile" ? "学习画像" : mode === "signin" ? "Connect" : action === "translate" ? "Translate" : "Triage";
  return (
    <header className="card-header" data-tauri-drag-region>
      <div className="header-title">
        {mode === "history" || mode === "settings" || mode === "profile" ? <button className="icon-button" aria-label="Back" onClick={onBack}><ArrowLeft size={17} /></button> : <span className="brand-glyph" aria-hidden="true"><img src={glossLogo} alt="" /></span>}
        <div><strong>{title}</strong>{mode !== "card" && mode !== "settings" && mode !== "profile" && <span>Gloss</span>}</div>
      </div>
      <div className="header-actions">
        {mode === "card" && <><button className="icon-button" aria-label="Open history" onClick={onHistory}><Clock3 size={17} /></button><button className="icon-button" aria-label="Open settings" onClick={onSettings}><Settings size={17} /></button></>}
        <button className="icon-button" aria-label="Close Gloss" onClick={onClose}><X size={17} /></button>
      </div>
    </header>
  );
}

type ReadingSelection = { text: string; left: number; top: number; placement: "above" | "below" };
type ResultProps = { session: Session | null; selection: Selection | null; streamText: string; busy: boolean; error: string | null; question: string; copied: boolean; onQuestionChange: (value: string) => void; onQuestionSubmit: (event: FormEvent) => void; onExplainSelection: (selectedText: string) => void; onRetry: () => void; onCopy: () => void };
const STREAM_END_TOLERANCE = 2;

function ResultPanel({ session, selection, streamText, busy, error, question, copied, onQuestionChange, onQuestionSubmit, onExplainSelection, onRetry, onCopy }: ResultProps) {
  const scrollElement = useRef<HTMLDivElement | null>(null);
  const followsStream = useRef(true);
  const popover = useRef<HTMLButtonElement | null>(null);
  const [readingSelection, setReadingSelection] = useState<ReadingSelection | null>(null);
  const messages = session?.messages.filter((_, index) => index > 0) ?? [];
  const hasAnswer = messages.some((message) => message.role === "assistant") || Boolean(streamText);
  const canExplain = session?.action === "triage" && hasAnswer && !busy;

  useLayoutEffect(() => {
    const element = scrollElement.current;
    if (!busy || !followsStream.current || !element) return;
    element.scrollTop = element.scrollHeight;
  }, [streamText, busy]);

  useEffect(() => {
    if (!readingSelection) return;
    const dismiss = (event: globalThis.PointerEvent) => {
      if (!popover.current?.contains(event.target as Node)) setReadingSelection(null);
    };
    const dismissWithEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      setReadingSelection(null);
    };
    document.addEventListener("pointerdown", dismiss);
    window.addEventListener("keydown", dismissWithEscape, true);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      window.removeEventListener("keydown", dismissWithEscape, true);
    };
  }, [readingSelection]);

  function handleResultScroll() {
    setReadingSelection(null);
    const element = scrollElement.current;
    if (!element) return;
    const distanceFromEnd = element.scrollHeight - element.clientHeight - element.scrollTop;
    followsStream.current = distanceFromEnd <= STREAM_END_TOLERANCE;
  }

  function captureReadingSelection() {
    if (!canExplain) {
      setReadingSelection(null);
      return;
    }
    const current = window.getSelection();
    if (!current || current.isCollapsed || current.rangeCount === 0) {
      setReadingSelection(null);
      return;
    }
    const text = current.toString().trim();
    const range = current.getRangeAt(0);
    const start = range.startContainer.nodeType === Node.ELEMENT_NODE ? range.startContainer as Element : range.startContainer.parentElement;
    const end = range.endContainer.nodeType === Node.ELEMENT_NODE ? range.endContainer as Element : range.endContainer.parentElement;
    const startSurface = start?.closest(".markdown-answer, .source-quote");
    const endSurface = end?.closest(".markdown-answer, .source-quote");
    if (!text || !startSurface || startSurface !== endSurface || !scrollElement.current?.contains(startSurface)) {
      setReadingSelection(null);
      return;
    }
    const rect = range.getBoundingClientRect();
    const placement = rect.top >= 112 ? "above" : "below";
    setReadingSelection({
      text,
      left: Math.min(window.innerWidth - 68, Math.max(68, rect.left + rect.width / 2)),
      top: placement === "above" ? rect.top - 8 : rect.bottom + 8,
      placement,
    });
  }

  function explainReadingSelection() {
    if (!readingSelection) return;
    const selectedText = readingSelection.text;
    setReadingSelection(null);
    window.getSelection()?.removeAllRanges();
    onExplainSelection(selectedText);
  }

  return <>
    <div className="result-scroll" ref={scrollElement} onPointerUp={() => window.requestAnimationFrame(captureReadingSelection)} onKeyUp={captureReadingSelection} onScroll={handleResultScroll}>
      {(session?.selectedText || selection?.text) && <blockquote className="source-quote"><span>Selected text</span><p>{session?.selectedText || selection?.text}</p></blockquote>}
      {!hasAnswer && busy && <LoadingState />}
      <div className="conversation">
        {messages.map((message) => message.role === "user" ? message.intent === "explain_selection" ? <div className="user-question explain-question" key={message.id}><span><MessageCircleQuestion size={13} /> Explain this</span><q>{message.content}</q></div> : <div className="user-question" key={message.id}>{message.content}</div> : <MarkdownAnswer key={message.id} content={message.content} />)}
        {streamText && <MarkdownAnswer content={streamText} streaming />}
      </div>
      {error && <div className="inline-error" role="alert"><div><strong>{streamText ? "Response interrupted" : "Couldn't complete this"}</strong><p>{error}</p></div><button className="secondary-button" onClick={onRetry} disabled={busy}><RefreshCw size={15} /> Retry</button></div>}
    </div>
    {readingSelection && <button ref={popover} className={`selection-popover is-${readingSelection.placement}`} style={{ left: readingSelection.left, top: readingSelection.top }} onPointerDown={(event) => event.preventDefault()} onClick={explainReadingSelection}><MessageCircleQuestion size={14} /> Explain this</button>}
    {(hasAnswer || error) && <footer className="result-footer">
      <div className="answer-tools"><button className="icon-button" onClick={onCopy} aria-label="Copy latest answer">{copied ? <Check size={16} /> : <Copy size={16} />}</button><span>{copied ? "Copied" : busy ? "Writing…" : "Response complete"}</span></div>
      {session?.action === "triage" && <form className="follow-up" onSubmit={onQuestionSubmit}><label htmlFor="follow-up" className="sr-only">Ask a follow-up</label><input id="follow-up" value={question} onChange={(event) => onQuestionChange(event.currentTarget.value)} placeholder="Ask about this text…" disabled={busy} autoComplete="off" /><button type="submit" aria-label="Send follow-up" disabled={busy || !question.trim()}><Send size={16} /></button></form>}
    </footer>}
  </>;
}

function MarkdownAnswer({ content, streaming = false }: { content: string; streaming?: boolean }) {
  return <article className={`markdown-answer${streaming ? " is-streaming" : ""}`}><ReactMarkdown remarkPlugins={[remarkGfm, remarkCjkStrongBoundaries]}>{content}</ReactMarkdown>{streaming && <span className="stream-caret" aria-hidden="true" />}</article>;
}
function LoadingState() { return <div className="loading-state" aria-label="Gloss is thinking"><div className="thinking-mark"><Sparkles size={18} /></div><div className="loading-lines"><i /><i /><i /></div></div>; }
function SignInPanel({ onSignIn, error, notice }: { onSignIn: () => void; error: string | null; notice: string }) {
  return <div className="center-panel"><div className="connect-mark"><LogIn size={24} /></div><h1>Connect ChatGPT</h1><p>Gloss uses your ChatGPT Codex access. Your sign-in is stored locally as plaintext in Gloss app data.</p><button className="primary-button wide" onClick={onSignIn}>Continue with ChatGPT <ChevronRight size={17} /></button>{(error || notice) && <p className={error ? "panel-error" : "panel-notice"}>{error || notice}</p>}<small>Your selected text is sent only when you invoke Gloss.</small></div>;
}

function HistoryPanel({ items, error, onOpen, onDelete }: { items: SessionSummary[]; error: string | null; onOpen: (id: string) => void; onDelete: (event: MouseEvent, id: string) => void }) {
  return <div className="panel-scroll history-panel"><p className="panel-intro">Your reading sessions stay on this PC.</p>{error && <p className="panel-error">{error}</p>}{!error && items.length === 0 && <div className="empty-state"><Clock3 size={22} /><strong>No history yet</strong><p>Your first Triage or Translate session will appear here.</p></div>}<div className="history-list">{items.map((item) => <div className="history-item" key={item.sessionId}><button className="history-open" onClick={() => onOpen(item.sessionId)}><span className={`history-icon ${item.action}`}>{item.action === "triage" ? <MessageCircleQuestion size={16} /> : <Languages size={16} />}</span><span className="history-copy"><strong>{item.preview}</strong><small>{formatTime(item.updatedAt)} · {capitalize(item.action)}</small></span></button><button className="history-delete" aria-label="Delete history item" onClick={(event) => onDelete(event, item.sessionId)}><Trash2 size={15} /></button></div>)}</div></div>;
}

const PROFILE_DIMENSIONS: ReadonlyArray<{ value: ProfileDimensionName; label: string }> = [
  { value: "reading", label: "阅读" },
  { value: "vocabulary", label: "词汇" },
  { value: "grammar", label: "语法" },
  { value: "pragmatics", label: "语用" },
];

function ProfilePanel({ value, loading, error }: { value: LearnerProfile | null; loading: boolean; error: string | null }) {
  if (loading) return <div className="panel-scroll profile-panel"><div className="empty-state"><UserRound size={22} /><strong>正在读取画像…</strong></div></div>;
  if (error) return <div className="panel-scroll profile-panel"><div className="empty-state"><UserRound size={22} /><strong>无法读取画像</strong><p>{error}</p></div></div>;
  if (!value || value.conversationsObserved === 0) return <div className="panel-scroll profile-panel"><div className="empty-state"><UserRound size={22} /><strong>尚未形成画像</strong><p>在 Triage 后继续提问，Gloss 会据此更新画像。</p></div></div>;

  const dimensions = new Map(value.dimensions.map((dimension) => [dimension.dimension, dimension]));
  const levelAvailable = value.overall.level !== "insufficient_evidence";
  return <div className="panel-scroll profile-panel">
    <section className="profile-summary" aria-labelledby="profile-overall-heading">
      <div className="profile-overview">
        <span className={`profile-level${levelAvailable ? "" : " is-pending"}`} aria-hidden="true">{levelAvailable ? value.overall.level : "—"}</span>
        <div className="profile-overall-copy"><span id="profile-overall-heading">综合水平</span><strong>{cefrDescription(value.overall.level)}</strong></div>
      </div>
      <p>{value.overall.rationale}</p>
      <div className="profile-meta"><span>{confidenceLabel(value.overall.confidence)}</span><span>{conversationLabel(value.conversationsObserved)}</span><span>更新于 {formatProfileDate(value.updatedAt)}</span></div>
    </section>

    <section className="profile-section" aria-labelledby="profile-skills-heading">
      <h2 id="profile-skills-heading">分项能力</h2>
      <div className="profile-dimensions">{PROFILE_DIMENSIONS.map(({ value: dimensionName, label }) => {
        const estimate = dimensions.get(dimensionName);
        return <article className="profile-dimension" key={dimensionName}>
          <div className="profile-dimension-heading"><strong>{label}</strong><span>{estimate ? cefrLabel(estimate.level) : "证据不足"}</span></div>
          {estimate && <><small>{confidenceLabel(estimate.confidence)}</small><p>{estimate.evidence}</p></>}
        </article>;
      })}</div>
    </section>

    {value.recentObservations.length > 0 && <section className="profile-section" aria-labelledby="profile-evidence-heading">
      <h2 id="profile-evidence-heading">最近证据</h2>
      <div className="profile-observations">{value.recentObservations.slice(0, 8).map((observation, index) => <article className="profile-observation" key={`${observation.dimension}-${observation.lastSeenAt}-${index}`}>
        <div className="profile-observation-meta"><span>{profileDimensionLabel(observation.dimension)}</span><span>{cefrLabel(observation.cefrLevel)}</span></div>
        <strong>{observation.descriptor}</strong>
        <p>{observation.evidence}</p>
        <small>{observation.evidenceCount} 条证据 · {formatProfileDate(observation.lastSeenAt)}</small>
      </article>)}</div>
    </section>}
  </div>;
}

type SettingsProps = { value: SettingsValue; auth: AuthStatus; error: string | null; saveState: SettingsSaveState; onChange: (value: SettingsValue) => void; onProfile: () => void; onSignIn: () => void; onSignOut: () => void };
const THEME_OPTIONS: ReadonlyArray<{ value: Theme; label: string }> = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

function ThemePicker({ value, onChange }: { value: Theme; onChange: (theme: Theme) => void }) {
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(() => THEME_OPTIONS.findIndex((option) => option.value === value));
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const selectedIndex = Math.max(0, THEME_OPTIONS.findIndex((option) => option.value === value));
  const selected = THEME_OPTIONS[selectedIndex];

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => optionRefs.current[activeIndex]?.focus());
    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutsideClick);
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener("pointerdown", closeOnOutsideClick);
    };
  }, [open]);

  function openPicker(index: number) {
    setActiveIndex(index);
    setOpen(true);
  }
  function closePicker(restoreFocus = false) {
    setOpen(false);
    if (restoreFocus) window.requestAnimationFrame(() => triggerRef.current?.focus());
  }
  function choose(index: number) {
    onChange(THEME_OPTIONS[index].value);
    closePicker(true);
  }
  function moveTo(index: number) {
    const nextIndex = (index + THEME_OPTIONS.length) % THEME_OPTIONS.length;
    setActiveIndex(nextIndex);
    optionRefs.current[nextIndex]?.focus();
  }
  function handleTriggerKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      openPicker(event.key === "ArrowUp" ? THEME_OPTIONS.length - 1 : selectedIndex);
    }
  }
  function handleOptionKeyDown(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    if (event.key === "ArrowDown") { event.preventDefault(); moveTo(index + 1); }
    else if (event.key === "ArrowUp") { event.preventDefault(); moveTo(index - 1); }
    else if (event.key === "Home") { event.preventDefault(); moveTo(0); }
    else if (event.key === "End") { event.preventDefault(); moveTo(THEME_OPTIONS.length - 1); }
    else if (event.key === "Enter" || event.key === " ") { event.preventDefault(); choose(index); }
    else if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); closePicker(true); }
    else if (event.key === "Tab") setOpen(false);
  }

  return <div className={`theme-picker${open ? " is-open" : ""}`} ref={rootRef} onBlur={(event) => {
    if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }}>
    <button
      id="theme"
      className="theme-trigger"
      type="button"
      ref={triggerRef}
      aria-haspopup="listbox"
      aria-expanded={open}
      aria-controls={open ? "theme-options" : undefined}
      aria-labelledby="theme-label theme-value"
      onClick={() => open ? closePicker() : openPicker(selectedIndex)}
      onKeyDown={handleTriggerKeyDown}
    >
      <span id="theme-value">{selected.label}</span>
      <ChevronDown size={14} aria-hidden="true" />
    </button>
    {open && <div id="theme-options" className="theme-options" role="listbox" aria-labelledby="theme-label">
      {THEME_OPTIONS.map((option, index) => <button
        className="theme-option"
        type="button"
        role="option"
        aria-selected={value === option.value}
        tabIndex={index === activeIndex ? 0 : -1}
        ref={(element) => { optionRefs.current[index] = element; }}
        key={option.value}
        onClick={() => choose(index)}
        onMouseMove={() => setActiveIndex(index)}
        onKeyDown={(event) => handleOptionKeyDown(event, index)}
      >
        <span>{option.label}</span>
        <Check size={14} aria-hidden="true" />
      </button>)}
    </div>}
  </div>;
}

function SettingsPanel({ value, auth, error, saveState, onChange, onProfile, onSignIn, onSignOut }: SettingsProps) {
  function recordShortcut(event: KeyboardEvent<HTMLInputElement>) {
    event.preventDefault();
    if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) return;
    const modifiers = [event.ctrlKey && "ctrl", event.altKey && "alt", event.shiftKey && "shift", event.metaKey && "super"].filter(Boolean);
    const key = normalizeKey(event.key);
    if (modifiers.length && key) onChange({ ...value, shortcut: [...modifiers, key].join("+") });
  }
  function updateProxy(proxyUrl: string) {
    onChange({ ...value, proxyUrl });
  }
  const connectionLabel = auth.status === "connected" ? "Connected" : auth.status === "error" ? "Connection failed" : "Not connected";
  const proxyError = saveState.status === "error" ? validateProxy(value.proxyUrl) : "";
  return <div className="panel-scroll settings-panel">
    <div className="settings-fields">
      <div className="setting-field setting-field-inline account-setting"><span className="account-copy"><strong className="setting-label">ChatGPT</strong><span className={`connection-status is-${auth.status}`}><i aria-hidden="true" />{connectionLabel}</span></span><button className="text-button" type="button" onClick={auth.status === "connected" ? onSignOut : onSignIn}>{auth.status === "connected" ? <><LogOut size={15} /> Sign out</> : <><LogIn size={15} /> Sign in</>}</button></div>
      <button className="profile-setting-link" type="button" onClick={onProfile}><strong className="setting-label">学习画像</strong><ChevronRight size={16} aria-hidden="true" /></button>
      <label className="setting-field" htmlFor="shortcut"><strong className="setting-label">Global shortcut</strong><input id="shortcut" className="shortcut-input" value={displayShortcut(value.shortcut)} onKeyDown={recordShortcut} onChange={() => undefined} /></label>
      <label className="setting-field" htmlFor="proxy-url"><strong className="setting-label">Proxy</strong><input id="proxy-url" name="proxy" className="network-input" type="text" inputMode="url" autoComplete="off" spellCheck={false} placeholder="127.0.0.1:23458" value={value.proxyUrl} onChange={(event) => updateProxy(event.target.value)} aria-invalid={proxyError ? true : undefined} aria-describedby={proxyError ? "proxy-error" : undefined} />{proxyError && <small id="proxy-error" className="field-error">{proxyError}</small>}</label>
      <div className="setting-field setting-field-inline theme-setting-row"><strong className="setting-label" id="theme-label">Theme</strong><ThemePicker value={value.theme} onChange={(theme) => onChange({ ...value, theme })} /></div>
    </div>
    <div className="settings-save-status" role="status" aria-live="polite">{saveState.status === "saving" ? <span className="is-saving"><i aria-hidden="true" />Saving…</span> : saveState.status === "saved" ? <span><Check size={13} aria-hidden="true" />Saved</span> : saveState.status === "error" ? <span className="is-error">{saveState.message}</span> : null}</div>
    {error && <p className="panel-error">{error}</p>}
  </div>;
}

function errorMessage(cause: unknown) { return typeof cause === "string" ? cause : cause instanceof Error ? cause.message : "Something went wrong."; }
function settingsEqual(left: SettingsValue, right: SettingsValue) { return left.shortcut === right.shortcut && left.theme === right.theme && left.proxyUrl === right.proxyUrl; }
function capitalize(value: string) { return value.charAt(0).toUpperCase() + value.slice(1); }
function formatTime(value: string) { return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(new Date(value)); }
function formatProfileDate(value: string) { return new Intl.DateTimeFormat("zh-CN", { month: "short", day: "numeric" }).format(new Date(value)); }
function cefrLabel(level: CefrLevel) { return level === "insufficient_evidence" ? "证据不足" : level; }
function cefrDescription(level: CefrLevel) { const descriptions: Record<CefrLevel, string> = { A1: "入门", A2: "基础", B1: "中级", B2: "中高级", C1: "高级", C2: "精通", insufficient_evidence: "证据不足" }; return descriptions[level]; }
function confidenceLabel(confidence: number) { return `置信度 ${Math.round(confidence * 100)}%`; }
function conversationLabel(count: number) { return `基于 ${count} 次对话`; }
function profileDimensionLabel(dimension: ProfileDimensionName) { return PROFILE_DIMENSIONS.find((item) => item.value === dimension)?.label ?? dimension; }
function displayShortcut(value: string) { const names: Record<string, string> = { ctrl: "Ctrl", alt: "Alt", shift: "Shift", super: "Win" }; return value.split("+").map((part) => names[part] ?? part.toUpperCase()).join(" + "); }
function normalizeKey(key: string) { if (key === " ") return "space"; if (key === "Escape") return "esc"; if (key.length === 1 && /[a-z0-9]/i.test(key)) return key.toLowerCase(); if (/^F\d{1,2}$/i.test(key)) return key.toLowerCase(); const named: Record<string, string> = { ArrowUp: "up", ArrowDown: "down", ArrowLeft: "left", ArrowRight: "right", Enter: "enter", Tab: "tab", Backspace: "backspace", Delete: "delete", Home: "home", End: "end", PageUp: "pageup", PageDown: "pagedown" }; return named[key] ?? ""; }
function validateProxy(value: string) {
  const raw = value.trim();
  if (!raw) return "";
  try {
    const url = new URL(raw.includes("://") ? raw : `http://${raw}`);
    if (!["http:", "https:", "socks5:", "socks5h:"].includes(url.protocol)) return "Use an HTTP, HTTPS, SOCKS5, or SOCKS5H proxy.";
    if (!url.hostname) return "Enter a host name or IP address.";
    if ((url.pathname && url.pathname !== "/") || url.search || url.hash) return "Remove the path, query, or fragment from the proxy URL.";
    return "";
  } catch {
    return "Enter a valid proxy, such as 127.0.0.1:23458.";
  }
}

export default App;
