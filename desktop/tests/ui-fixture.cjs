async (page) => {
  await page.addInitScript(() => {
    let state = { model: 'fake', effort: 'off', permissionMode: 'default', sessionId: null, taskPending: false, rounds: 0, contextTokens: 18400, contextWindow: 200000, contextBreakdown: { system: 6200, tools: 9800, messages: 2400 }, usage: { input_tokens: 42000, output_tokens: 3100 }, budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 }, acceptance: '', alwaysAllow: [] };
    const workspace = 'D:\\miniclaudecode';
    const now = Date.now();
    let sessions = [
      { session_id: 'audit', title: '审查工作区的未提交改动', workspace, status: 'completed', created_at: new Date(now - 3600000).toISOString(), updated_at: new Date(now - 600000).toISOString(), pinned: false },
      { session_id: 'bridge', title: '完善 Agent 事件流与任务进度', workspace, status: 'completed', created_at: new Date(now - 86400000).toISOString(), updated_at: new Date(now - 86400000).toISOString(), pinned: true },
      { session_id: 'terminal', title: '排查终端输出的刷新问题', workspace, status: 'paused', created_at: new Date(now - 172800000).toISOString(), updated_at: new Date(now - 172800000).toISOString(), pinned: false },
    ];
    const models = [{ id: 'fake', provider: 'offline', available: true, supportsEffort: false }, { id: 'deepseek/deepseek-v4.1-flash', provider: 'commandcode', available: true, supportsEffort: true }, { id: 'claude-sonnet-4-6', provider: 'anthropic', available: false, supportsEffort: false }];
    const commands = [{ name: '/help', summary: '查看命令' }, { name: '/model', summary: '切换模型' }, { name: '/new', summary: '新建任务' }, { name: '/clear', summary: '清空视图' }, { name: '/compact', summary: '压缩上下文' }];
    const at = offset => new Date(now - offset).toISOString();
    const events = [
      { seq: 1, type: 'tool_call_start', timestamp: at(9000), data: { call_id: 'c1', name: 'read' } },
      { seq: 2, type: 'tool_call_result', timestamp: at(8600), data: { call_id: 'c1', success: true, output_detail: '1\texport default function App() {' } },
      { seq: 3, type: 'tool_call_start', timestamp: at(8000), data: { call_id: 'c2', name: 'grep' } },
      { seq: 4, type: 'tool_call_result', timestamp: at(7600), data: { call_id: 'c2', success: true, output_detail: 'desktop/src/App.tsx:12: useState' } },
      { seq: 5, type: 'tool_call_start', timestamp: at(7000), data: { call_id: 'c3', name: 'edit' } },
      { seq: 6, type: 'tool_call_result', timestamp: at(6200), data: { call_id: 'c3', success: true, output_detail: 'edited desktop/src/App.tsx' } },
      { seq: 7, type: 'tool_call_start', timestamp: at(6000), data: { call_id: 'c4', name: 'bash' } },
      { seq: 8, type: 'tool_call_result', timestamp: at(2500), data: { call_id: 'c4', success: true, output_detail: '> tsc --noEmit && vite build\n✓ built in 3.2s' } },
    ];
    const messages = [
      { role: 'user', content: [{ type: 'text', text: '帮我审查当前工作区的改动，重点关注事件流与界面交互。' }] },
      { role: 'assistant', content: [
        { type: 'tool_use', id: 'c1', name: 'read', input: { path: 'desktop/src/App.tsx' } },
        { type: 'tool_use', id: 'c2', name: 'grep', input: { pattern: 'useState', path: 'desktop/src' } },
        { type: 'tool_use', id: 'c3', name: 'edit', input: { path: 'desktop/src/App.tsx', old_text: 'const [theme] = useState("dark")', new_text: 'const [theme, setTheme] = useState("system")\n// Follow the system appearance' } },
        { type: 'tool_use', id: 'c4', name: 'bash', input: { command: 'npm run build' } },
      ] },
      { role: 'assistant', content: [{ type: 'text', text: '### 审查结果\n\n已检查事件流与界面更新，主要涉及以下文件：\n\n- `App.tsx`：流式消息按批更新，降低渲染频率。\n- `WorkPanel.tsx`：切换文件时丢弃过期的读取结果。\n\n```tsx\nconst active = useRef(true)\nif (active.current) setContent(result)\n```\n\n| 文件 | 结论 |\n| --- | --- |\n| App.tsx | 通过 |\n\n建议继续检查**连续切换工作区**时的任务状态。' }] },
    ];
    let listener = () => {};
    window.__emit = event => listener(event);
    window.__requests = [];
    window.desktop = {
      onEvent: fn => { listener = fn; return () => { listener = () => {}; }; },
      chooseWorkspace: async () => workspace,
      chooseAcceptanceFile: async () => null,
      getPathForFile: () => '',
      request: async (method, params = {}) => {
        window.__requests.push({ method, params });
        if (method === 'getWorkspace') return workspace;
        if (method === 'initialize') return { sessions, models, commands, state };
        if (method === 'listSessions') return sessions;
        if (method === 'renameSession') { sessions = sessions.map(s => s.session_id === params.sessionId ? { ...s, title: params.title, custom_title: true } : s); return sessions; }
        if (method === 'pinSession') { sessions = sessions.map(s => s.session_id === params.sessionId ? { ...s, pinned: params.pinned } : s); return sessions; }
        if (method === 'deleteSession') { sessions = sessions.filter(s => s.session_id !== params.sessionId); return sessions; }
        if (method === 'listFiles') return ['desktop/src/App.tsx', 'desktop/src/styles/index.css', 'desktop/src/components/Sidebar.tsx', 'desktop/src/components/feed/SessionFeed.tsx', 'desktop/src/components/work/WorkPanel.tsx', 'desktop/bridge.py', 'README.md'];
        if (method === 'changes') return [{ path: 'desktop/src/App.tsx', status: ' M', additions: 12, deletions: 3 }, { path: 'desktop/src/styles/index.css', status: '??', additions: 40, deletions: 0 }, { path: 'desktop/src/components/Sidebar.tsx', status: 'M ', additions: 5, deletions: 5 }];
        if (method === 'getCapabilities') return { skills: [], plugins: [], mcp: [], agents: ['explore', 'review'] };
        if (method === 'listTasks') return params.sessionId === 'audit' ? [{ task_id: 't1', title: '读取改动文件', status: 'done' }, { task_id: 't2', title: '运行构建', status: 'done' }, { task_id: 't3', title: '整理结论', status: 'running' }] : [];
        if (method === 'getAppInfo') return { version: '0.1.0', electron: '44.2.0', chrome: '140', platform: 'win32' };
        if (method === 'relativePath') return null;
        if (method === 'notify' || method === 'openExternal' || method === 'setAppearance' || method === 'stageAll' || method === 'confirmDiff' || method === 'unstageFile') return true;
        if (method === 'setModel') state = { ...state, model: params.model };
        if (method === 'setEffort') state = { ...state, effort: params.effort };
        if (method === 'setPermissionMode') state = { ...state, permissionMode: params.mode };
        if (method === 'resolveApproval' && params.remember) state = { ...state, alwaysAllow: ['bash'] };
        if (method === 'clearAlwaysAllow') state = { ...state, alwaysAllow: [] };
        if (method === 'selectSession') state = { ...state, sessionId: params.sessionId };
        if (method === 'resetSession') state = { ...state, sessionId: null };
        if (method === 'diff') return 'diff --git a/desktop/src/App.tsx b/desktop/src/App.tsx\n--- a/desktop/src/App.tsx\n+++ b/desktop/src/App.tsx\n@@ -12,3 +12,4 @@\n export default function App() {\n-  const [theme] = useState("dark")\n+  const [theme, setTheme] = useState("system")\n+  // Follow the system appearance\n }';
        if (method === 'readFile') return 'import { useState } from "react"\n\nexport default function App() {\n  return <main>MiniCode</main>\n}\n';
        if (method === 'getSession') return { summary: sessions.find(s => s.session_id === params.sessionId), events: params.sessionId === 'audit' ? events : [], messages: params.sessionId === 'audit' ? messages : messages.slice(0, 1).concat(messages.slice(2)) };
        if (method === 'sendPrompt') return { sessionId: state.sessionId || 'test-run' };
        if (method === 'cancelTurn') { listener({ event: 'run_done', sessionId: 'test-run', result: { exit_reason: 'cancelled' } }); return true; }
        return state;
      }
    };
  });
  await page.setViewportSize({ width: 1500, height: 940 });
  // Reuse the origin of an already-open dev page (any port); default to Vite's 5173.
  const current = page.url();
  await page.goto(current.startsWith('http://127.0.0.1') ? new URL(current).origin : 'http://127.0.0.1:5173');
  await page.getByRole('textbox', { name: '任务输入' }).waitFor();
}
