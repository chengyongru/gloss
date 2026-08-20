import { forwardRef, useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent, type MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowLeft,
  Check,
  ChevronRight,
  Clock3,
  Copy,
  Grip,
  Languages,
  LogIn,
  LogOut,
  MessageCircleQuestion,
  RefreshCw,
  Send,
  Settings,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import "./App.css";

type Action = "triage" | "translate";
type Mode = "toolbar" | "card" | "history" | "settings" | "signin";
type Theme = "system" | "light" | "dark";
type Selection = { text: string; anchor?: { left: number; top: number; width: number; height: number } };
type OverlayState = { status: "idle" } | { status: "ready"; selection: Selection } | { status: "capture_error"; message: string };
type Message = { id: string; turnId: string; role: "user" | "assistant"; content: string; timestamp: string; attemptId?: string };
type Session = { sessionId: string; action: Action; selectedText: string; messages: Message[] };
type SessionSummary = { sessionId: string; action: Action; preview: string; updatedAt: string };
type SettingsValue = { shortcut: string; saveHistory: boolean; theme: Theme; proxyUrl: string };
type AuthStatus = { status: "signed_out" } | { status: "connected"; expiresAtMs: number } | { status: "error"; message: string };
type AgentEvent =
  | { type: "started"; sessionId: string; attemptId: string }
  | { type: "delta"; sessionId: string; attemptId: string; delta: string }
  | { type: "completed"; sessionId: string; attemptId: string; session: Session }
  | { type: "failed"; sessionId: string; attemptId: string; message: string; partial: string; incomplete: boolean };

const DEFAULT_SETTINGS: SettingsValue = { shortcut: "ctrl+alt+shift+t", saveHistory: true, theme: "system", proxyUrl: "" };
const isTauri = "__TAURI_INTERNALS__" in window;
const DEMO_SESSION: Session = {
  sessionId: "preview",
  action: "triage",
  selectedText: "pleased to announce ive been awarded the title of most obscure and forgotten former fyad",
  messages: [
    { id: "u", turnId: "t", role: "user", content: "pleased to announce ive been awarded the title of most obscure and forgotten former fyad", timestamp: new Date().toISOString() },
    { id: "a", turnId: "t", role: "assistant", content: "## Quick read\n\n很高兴宣布，我被授予了“最默默无闻、最被遗忘的前 FYAD 成员”这一称号。语气带有明显的自嘲和网络幽默。\n\n## What makes it hard\n\n### Usage · ive\n\n`ive` 是聊天中的非正式拼写，标准写法是 `I've`。\n\n### Grammar · been awarded\n\n这是现在完成时的被动语态：`have been + past participle`，强调已经发生且与现在相关的结果。\n\n### Culture · mock award\n\n把负面评价包装成正式奖项，是一种夸张的自嘲。\n\n## In plain English\n\nI'm happy to say that people jokingly named me the least-known and most-forgotten former FYAD member.", timestamp: new Date().toISOString() },
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
  const [question, setQuestion] = useState("");
  const [copied, setCopied] = useState(false);
  const [notice, setNotice] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

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

  useEffect(() => { document.documentElement.dataset.theme = settings.theme; }, [settings.theme]);
  useEffect(() => { if (busy) scrollRef.current?.scrollTo({ top: scrollRef.current.scrollHeight }); }, [streamText, busy]);
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

  async function retry() {
    setBusy(true); setError(null); setStreamText("");
    try { await invoke("retry_turn"); }
    catch (cause) { setBusy(false); setError(errorMessage(cause)); }
  }

  async function showHistory() {
    setMode("history"); setError(null);
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

  function showSettings() { setDraftSettings(settings); setError(null); setMode("settings"); }
  async function showSettingsFromToolbar() {
    setDraftSettings(settings);
    setError(null);
    setMode("settings");
    if (isTauri) await invoke("show_settings").catch((cause) => setError(errorMessage(cause)));
  }
  async function saveSettings(event: FormEvent) {
    event.preventDefault();
    try {
      const value = await invoke<SettingsValue>("update_settings", { settings: draftSettings });
      setSettings(value); setDraftSettings(value); setNotice("Settings saved"); setMode("card");
    } catch (cause) { setError(errorMessage(cause)); }
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
        <Toolbar selection={selection} error={captureError} onAction={runAction} onSettings={showSettingsFromToolbar} onClose={close} />
      ) : (
        <section className="card" aria-label="Gloss reading companion">
          <CardHeader mode={mode} action={session?.action ?? pendingAction} onBack={() => setMode("card")} onHistory={showHistory} onSettings={showSettings} onClose={close} />
          {mode === "signin" && <SignInPanel onSignIn={signIn} error={error} notice={notice} />}
          {mode === "history" && <HistoryPanel items={history} error={error} onOpen={openHistory} onDelete={removeHistory} />}
          {mode === "settings" && <SettingsPanel value={draftSettings} auth={auth} error={error} onChange={setDraftSettings} onSubmit={saveSettings} onSignIn={signIn} onSignOut={signOut} />}
          {mode === "card" && <ResultPanel ref={scrollRef} session={session} selection={selection} streamText={streamText} busy={busy} error={error} question={question} copied={copied} onQuestionChange={setQuestion} onQuestionSubmit={submitQuestion} onRetry={retry} onCopy={copyAnswer} />}
          <div className="sr-status" role="status" aria-live="polite">{notice || (busy ? "Gloss is thinking" : error ?? "")}</div>
        </section>
      )}
    </main>
  );
}

function Toolbar({ selection, error, onAction, onSettings, onClose }: { selection: Selection | null; error: string | null; onAction: (action: Action) => void; onSettings: () => void; onClose: () => void }) {
  return (
    <section className="toolbar" aria-label="Text actions" data-tauri-drag-region>
      <div className="toolbar-mark" aria-hidden="true"><Sparkles size={16} /></div>
      <p className={error ? "toolbar-error" : "selection-peek"} title={error ?? selection?.text}>{error ? "No readable selection" : selection?.text || "Selected text"}</p>
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
  const title = mode === "history" ? "History" : mode === "settings" ? "Settings" : mode === "signin" ? "Connect" : action === "translate" ? "Translate" : "Triage";
  return (
    <header className="card-header" data-tauri-drag-region>
      <div className="header-title">
        {mode === "history" || mode === "settings" ? <button className="icon-button" aria-label="Back" onClick={onBack}><ArrowLeft size={17} /></button> : <span className="brand-glyph" aria-hidden="true">G</span>}
        <div><strong>{title}</strong><span>{mode === "card" ? "English, made legible" : "Gloss"}</span></div>
      </div>
      <div className="header-actions">
        {mode === "card" && <><button className="icon-button" aria-label="Open history" onClick={onHistory} title="History"><Clock3 size={17} /></button><button className="icon-button" aria-label="Open settings" onClick={onSettings} title="Settings"><Settings size={17} /></button></>}
        <Grip className="drag-grip" size={16} aria-hidden="true" data-tauri-drag-region />
        <button className="icon-button" aria-label="Close Gloss" onClick={onClose}><X size={17} /></button>
      </div>
    </header>
  );
}

type ResultProps = { session: Session | null; selection: Selection | null; streamText: string; busy: boolean; error: string | null; question: string; copied: boolean; onQuestionChange: (value: string) => void; onQuestionSubmit: (event: FormEvent) => void; onRetry: () => void; onCopy: () => void };
const ResultPanel = forwardRef<HTMLDivElement, ResultProps>(function ResultPanel({ session, selection, streamText, busy, error, question, copied, onQuestionChange, onQuestionSubmit, onRetry, onCopy }, ref) {
  const messages = session?.messages.filter((_, index) => index > 0) ?? [];
  const hasAnswer = messages.some((message) => message.role === "assistant") || Boolean(streamText);
  return <>
    <div className="result-scroll" ref={ref}>
      {(session?.selectedText || selection?.text) && <blockquote className="source-quote"><span>Selected text</span><p>{session?.selectedText || selection?.text}</p></blockquote>}
      {!hasAnswer && busy && <LoadingState />}
      <div className="conversation">
        {messages.map((message) => message.role === "user" ? <div className="user-question" key={message.id}>{message.content}</div> : <MarkdownAnswer key={message.id} content={message.content} />)}
        {streamText && <MarkdownAnswer content={streamText} streaming />}
      </div>
      {error && <div className="inline-error" role="alert"><div><strong>{streamText ? "Response interrupted" : "Couldn't complete this"}</strong><p>{error}</p></div><button className="secondary-button" onClick={onRetry} disabled={busy}><RefreshCw size={15} /> Retry</button></div>}
    </div>
    {(hasAnswer || error) && <footer className="result-footer">
      <div className="answer-tools"><button className="icon-button" onClick={onCopy} aria-label="Copy latest answer">{copied ? <Check size={16} /> : <Copy size={16} />}</button><span>{copied ? "Copied" : busy ? "Writing…" : "Response complete"}</span></div>
      {session?.action === "triage" && <form className="follow-up" onSubmit={onQuestionSubmit}><label htmlFor="follow-up" className="sr-only">Ask a follow-up</label><input id="follow-up" value={question} onChange={(event) => onQuestionChange(event.currentTarget.value)} placeholder="Ask about this text…" disabled={busy} autoComplete="off" /><button type="submit" aria-label="Send follow-up" disabled={busy || !question.trim()}><Send size={16} /></button></form>}
    </footer>}
  </>;
});

function MarkdownAnswer({ content, streaming = false }: { content: string; streaming?: boolean }) {
  return <article className={`markdown-answer${streaming ? " is-streaming" : ""}`}><ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>{streaming && <span className="stream-caret" aria-hidden="true" />}</article>;
}
function LoadingState() { return <div className="loading-state" aria-label="Gloss is thinking"><div className="thinking-mark"><Sparkles size={18} /></div><div className="loading-lines"><i /><i /><i /></div></div>; }
function SignInPanel({ onSignIn, error, notice }: { onSignIn: () => void; error: string | null; notice: string }) {
  return <div className="center-panel"><div className="connect-mark"><LogIn size={24} /></div><h1>Connect ChatGPT</h1><p>Gloss uses your ChatGPT Codex access. Your sign-in is stored locally as plaintext in Gloss app data.</p><button className="primary-button wide" onClick={onSignIn}>Continue with ChatGPT <ChevronRight size={17} /></button>{(error || notice) && <p className={error ? "panel-error" : "panel-notice"}>{error || notice}</p>}<small>No API key, clipboard capture, or cloud sync.</small></div>;
}

function HistoryPanel({ items, error, onOpen, onDelete }: { items: SessionSummary[]; error: string | null; onOpen: (id: string) => void; onDelete: (event: MouseEvent, id: string) => void }) {
  return <div className="panel-scroll history-panel"><p className="panel-intro">Your reading sessions stay on this PC.</p>{error && <p className="panel-error">{error}</p>}{!error && items.length === 0 && <div className="empty-state"><Clock3 size={22} /><strong>No history yet</strong><p>Your first Triage or Translate session will appear here.</p></div>}<div className="history-list">{items.map((item) => <div className="history-item" key={item.sessionId}><button className="history-open" onClick={() => onOpen(item.sessionId)}><span className={`history-icon ${item.action}`}>{item.action === "triage" ? <MessageCircleQuestion size={16} /> : <Languages size={16} />}</span><span className="history-copy"><strong>{item.preview}</strong><small>{formatTime(item.updatedAt)} · {capitalize(item.action)}</small></span></button><button className="history-delete" aria-label="Delete history item" onClick={(event) => onDelete(event, item.sessionId)}><Trash2 size={15} /></button></div>)}</div></div>;
}

type SettingsProps = { value: SettingsValue; auth: AuthStatus; error: string | null; onChange: (value: SettingsValue) => void; onSubmit: (event: FormEvent) => void; onSignIn: () => void; onSignOut: () => void };
function SettingsPanel({ value, auth, error, onChange, onSubmit, onSignIn, onSignOut }: SettingsProps) {
  const proxyRef = useRef<HTMLInputElement>(null);
  const [proxyError, setProxyError] = useState("");

  function recordShortcut(event: KeyboardEvent<HTMLInputElement>) {
    event.preventDefault();
    if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) return;
    const modifiers = [event.ctrlKey && "ctrl", event.altKey && "alt", event.shiftKey && "shift", event.metaKey && "super"].filter(Boolean);
    const key = normalizeKey(event.key);
    if (modifiers.length && key) onChange({ ...value, shortcut: [...modifiers, key].join("+") });
  }
  function submit(event: FormEvent) {
    const message = validateProxy(value.proxyUrl);
    if (message) {
      event.preventDefault();
      setProxyError(message);
      proxyRef.current?.focus();
      return;
    }
    setProxyError("");
    onSubmit(event);
  }
  function updateProxy(proxyUrl: string) {
    if (proxyError) setProxyError("");
    onChange({ ...value, proxyUrl });
  }
  return <form className="panel-scroll settings-panel" onSubmit={submit} noValidate>
    <section className="setting-group"><h2>Activation</h2><label className="setting-row stacked" htmlFor="shortcut"><span><strong>Global shortcut</strong><small>Select text, then press this shortcut.</small></span><input id="shortcut" className="shortcut-input" value={displayShortcut(value.shortcut)} onKeyDown={recordShortcut} onChange={() => undefined} aria-describedby="shortcut-help" /><small id="shortcut-help">Click the field and press your new combination.</small></label></section>
    <section className="setting-group"><h2>Appearance & data</h2><label className="setting-row" htmlFor="theme"><span><strong>Theme</strong><small>Match Windows or choose one.</small></span><select id="theme" value={value.theme} onChange={(event) => onChange({ ...value, theme: event.target.value as Theme })}><option value="system">System</option><option value="light">Light</option><option value="dark">Dark</option></select></label><label className="setting-row" htmlFor="history-toggle"><span><strong>Save history</strong><small>Store complete sessions as local JSONL.</small></span><input id="history-toggle" className="switch" type="checkbox" checked={value.saveHistory} onChange={(event) => onChange({ ...value, saveHistory: event.target.checked })} /></label></section>
    <section className="setting-group"><h2>Network</h2><label className="setting-row stacked" htmlFor="proxy-url"><span><strong>Proxy</strong><small>Optional. Used for OAuth token exchange and all AI requests.</small></span><input ref={proxyRef} id="proxy-url" name="proxy" className="network-input" type="text" inputMode="url" autoComplete="off" spellCheck={false} placeholder="127.0.0.1:23458" value={value.proxyUrl} onChange={(event) => updateProxy(event.target.value)} aria-invalid={proxyError ? true : undefined} aria-describedby={`proxy-help${proxyError ? " proxy-error" : ""}`} /><small id="proxy-help">HTTP, HTTPS, SOCKS5, and SOCKS5H are supported.</small>{proxyError && <small id="proxy-error" className="field-error">{proxyError}</small>}</label></section>
    <section className="setting-group"><h2>ChatGPT</h2><div className="setting-row"><span><strong>{auth.status === "connected" ? "Connected" : auth.status === "error" ? "Connection failed" : "Not connected"}</strong><small>OAuth credentials are stored locally in plaintext.</small></span><button className="text-button" type="button" onClick={auth.status === "connected" ? onSignOut : onSignIn}>{auth.status === "connected" ? <><LogOut size={15} /> Sign out</> : <><LogIn size={15} /> Sign in</>}</button></div></section>
    {error && <p className="panel-error">{error}</p>}<button className="primary-button wide save-button" type="submit">Save settings</button>
  </form>;
}

function errorMessage(cause: unknown) { return typeof cause === "string" ? cause : cause instanceof Error ? cause.message : "Something went wrong."; }
function capitalize(value: string) { return value.charAt(0).toUpperCase() + value.slice(1); }
function formatTime(value: string) { return new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" }).format(new Date(value)); }
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
