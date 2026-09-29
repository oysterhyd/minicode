async (page) => {
  await page.addInitScript(() => {
    let state = { model: 'fake', effort: 'off', permissionMode: 'default', sessionId: null, taskPending: false, rounds: 0, contextTokens: 0, contextWindow: 200000, contextBreakdown: { system: 0, tools: 0, messages: 0 }, usage: null, budget: { max_rounds: 0, max_total_tokens: 0, max_seconds: 0 }, acceptance: '' };
    const workspace = 'D:\\miniclaudecode';
    const sessions = [
      { session_id: 'audit', title: '审查工作区的未提交改动', workspace, status: 'completed', created_at: new Date().toISOString() },
      { session_id: 'bridge', title: '完善 Agent 事件流与任务进度', workspace, status: 'completed', created_at: new Date(Date.now() - 86400000).toISOString() },
      { session_id: 'terminal', title: '排查终端输出的刷新问题', workspace, status: 'paused', created_at: new Date(Date.now() - 172800000).toISOString() },
    ];
    const models = [{ id: 'fake', provider: 'offline', available: true, supportsEffort: false }, { id: 'deepseek/deepseek-v4.1-flash', provider: 'commandcode', available: true, supportsEffort: true }, { id: 'claude-sonnet-4-6', provider: 'anthropic', available: false, supportsEffort: false }];
    const commands = [{ name: '/help', summary: '查看命令' }, { name: '/model', summary: '切换模型' }, { name: '/new', summary: '新建任务' }, { name: '/clear', summary: '清空视图' }];
    let listener = () => {};
    window.__emit = event => listener(event);
    window.__requests = [];
    window.desktop = {
      onEvent: fn => { listener = fn; return () => { listener = () => {}; }; },
      chooseWorkspace: async () => workspace,
      chooseAcceptanceFile: async () => null,
      request: async (method, params = {}) => {
        window.__requests.push({ method, params });
        if (method === 'getWorkspace') return workspace;
        if (method === 'initialize') return { sessions, models, commands, state };
        if (method === 'listSessions') return sessions;
        if (method === 'listFiles') return ['desktop/src/App.tsx', 'desktop/src/style.css', 'desktop/src/components/Sidebar.tsx', 'desktop/src/components/SessionFeed.tsx', 'desktop/src/components/WorkPanel.tsx', 'desktop/bridge.py', 'README.md'];
        if (method === 'changes') return [{ path: 'desktop/src/App.tsx', status: ' M' }, { path: 'desktop/src/style.css', status: ' M' }, { path: 'desktop/src/components/Sidebar.tsx', status: ' M' }];
        if (method === 'getCapabilities') return { skills: [], plugins: [], mcp: [], agents: ['explore', 'review'] };
        if (method === 'listTasks') return [];
        if (method === 'setModel') state = { ...state, model: params.model };
        if (method === 'setEffort') state = { ...state, effort: params.effort };
        if (method === 'setPermissionMode') state = { ...state, permissionMode: params.mode };
        if (method === 'selectSession') state = { ...state, sessionId: params.sessionId };
        if (method === 'resetSession') state = { ...state, sessionId: null };
        if (method === 'diff') return 'diff --git a/desktop/src/App.tsx b/desktop/src/App.tsx\n--- a/desktop/src/App.tsx\n+++ b/desktop/src/App.tsx\n@@ -12,3 +12,4 @@\n export default function App() {\n-  const [theme] = useState("dark")\n+  const [theme, setTheme] = useState("system")\n+  // Follow the system appearance\n }';
        if (method === 'readFile') return 'import { useState } from "react"\n\nexport default function App() {\n  return <main>MiniCode</main>\n}\n';
        if (method === 'getSession') return { summary: sessions.find(s => s.session_id === params.sessionId), events: [], messages: [{ role: 'user', content: [{ type: 'text', text: '帮我审查当前工作区的改动，重点关注事件流与界面交互。' }] }, { role: 'assistant', content: [{ type: 'text', text: '### 审查结果\n\n已检查事件流与界面更新，主要涉及以下文件：\n\n- `App.tsx`：流式消息按批更新，降低渲染频率。\n- `WorkPanel.tsx`：切换文件时丢弃过期的读取结果。\n\n```tsx\nconst active = useRef(true)\nif (active.current) setContent(result)\n```\n\n建议继续检查**连续切换工作区**时的任务状态。' }] }] };
        if (method === 'sendPrompt') return { sessionId: state.sessionId || 'test-run' };
        if (method === 'cancelTurn') { listener({ event: 'run_done', sessionId: 'test-run', result: { exit_reason: 'cancelled' } }); return true; }
        return state;
      }
    };
  });
  await page.setViewportSize({ width: 1500, height: 940 });
  await page.goto('http://127.0.0.1:5173');
  await page.getByRole('textbox', { name: '任务输入' }).waitFor();

}
