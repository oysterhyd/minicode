(() => {
  "use strict";

  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
  const ease = t => 1 - Math.pow(1 - t, 3);
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;

  /** Fires `on`/`off` as an element enters and leaves the viewport. */
  function watch(el, on, off, threshold = 0.35) {
    if (!el) return;
    new IntersectionObserver(([e]) => (e.isIntersecting ? on() : off && off()), { threshold }).observe(el);
  }

  /* ───────── Reveal on scroll ───────── */
  const revealer = new IntersectionObserver(entries => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add("in");
      revealer.unobserve(e.target);
    }
  }, { threshold: 0.15, rootMargin: "0px 0px -8% 0px" });
  $$(".reveal").forEach(el => revealer.observe(el));

  /* ───────── Count-up stats ───────── */
  $$("[data-count]").forEach(el => {
    const target = parseFloat(el.dataset.count), dec = +el.dataset.dec, pre = el.dataset.prefix || "";
    watch(el, () => {
      if (el.dataset.done) return;
      el.dataset.done = 1;
      const t0 = performance.now(), dur = reduce ? 1 : 1600;
      const tick = now => {
        const k = ease(clamp((now - t0) / dur));
        el.textContent = pre + (target * k).toFixed(dec);
        if (k < 1) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
  });

  /* ───────── Startup race ───────── */
  const race = $("#race");
  const runRace = () => { race.classList.remove("run"); void race.offsetWidth; race.classList.add("run"); };
  watch(race, () => race.classList.contains("run") || runRace());
  $("#raceBtn").addEventListener("click", runRace);

  /* ───────── Scroll-driven scenes ───────── */
  const nav = $("#nav"), navProgress = $("#navProgress");
  const hero = $("#hero"), heroCopy = $(".hero-copy"), stage = $(".hero-stage"), core = $(".core");
  const surfaces = $$(".surface").map(el => ({ el, x: +el.dataset.x, y: +el.dataset.y, r: +el.dataset.r }));
  const pipeSection = $("#pipeline"), pipe = $(".pipe"), stages = $$(".pipe-stages li"), notes = $$(".pipe-note span");
  const appStage = $("#appStage"), app = $("#app");
  const lightSections = $$(".light");
  const navLinks = $$(".nav-links a").map(a => ({ a, sec: $(a.getAttribute("href")) }));
  let mouse = { x: 0, y: 0 };

  const progress = el => {
    const r = el.getBoundingClientRect();
    return clamp(-r.top / (r.height - innerHeight));
  };

  function renderHero() {
    const p = progress(hero);
    const fade = clamp(p / 0.28);
    heroCopy.style.opacity = 1 - fade;
    heroCopy.style.transform = `translateY(${-fade * 120}px) scale(${1 - fade * 0.12})`;

    const q = ease(clamp((p - 0.12) / 0.5));
    const gone = clamp((p - 0.52) / 0.14);
    stage.style.setProperty("--lift", `${-q * 18}vh`);
    for (const s of surfaces) {
      const k = 1 - q;
      const px = mouse.x * (s.r ? s.r * 2.5 : 10) * k, py = mouse.y * 14 * k;
      s.el.style.transform =
        `translate(-50%, -50%) translate(${s.x * k + px}px, ${s.y * k + py}px) rotate(${s.r * k}deg) scale(${1 - q * 0.7})`;
      s.el.style.opacity = 1 - gone;
    }
    const c = ease(clamp((p - 0.5) / 0.3));
    core.style.opacity = c;
    core.style.transform = `translate(-50%, -50%) scale(${0.4 + c * 0.6})`;
  }

  function renderPipeline() {
    const p = clamp(progress(pipeSection) / 0.92);
    const idx = Math.min(5, Math.floor(p * 6));
    pipe.style.setProperty("--p", clamp((p * 6 - 0.5) / 5));
    pipe.className = `pipe at-${idx}`;
    stages.forEach((li, i) => { li.classList.toggle("done", i < idx); li.classList.toggle("now", i === idx); });
    notes.forEach((n, i) => n.classList.toggle("on", i === idx));
  }

  function renderApp() {
    const r = appStage.getBoundingClientRect();
    app.style.setProperty("--tilt", ease(clamp((r.top - innerHeight * 0.12) / (innerHeight * 0.6))));
  }

  function renderNav() {
    const doc = document.documentElement;
    navProgress.style.transform = `scaleX(${clamp(scrollY / (doc.scrollHeight - innerHeight))})`;
    const y = 52;
    nav.classList.toggle("on-light", lightSections.some(s => {
      const r = s.getBoundingClientRect();
      return r.top <= y && r.bottom > y;
    }));
    for (const { a, sec } of navLinks) {
      const r = sec.getBoundingClientRect();
      a.classList.toggle("active", r.top <= innerHeight * 0.4 && r.bottom > innerHeight * 0.4);
    }
  }

  let ticking = false;
  function frame() {
    ticking = false;
    renderHero(); renderPipeline(); renderApp(); renderNav();
  }
  const request = () => { if (!ticking) { ticking = true; requestAnimationFrame(frame); } };
  addEventListener("scroll", request, { passive: true });
  addEventListener("resize", () => { request(); $$(".seg").forEach(placePill); });
  frame();

  /* ───────── Hero pointer parallax + glow ───────── */
  const glow = $(".hero-glow");
  hero.addEventListener("pointermove", e => {
    mouse = { x: e.clientX / innerWidth - 0.5, y: e.clientY / innerHeight - 0.5 };
    glow.style.setProperty("--mx", `${e.clientX / innerWidth * 100}%`);
    glow.style.setProperty("--my", `${e.clientY / innerHeight * 100}%`);
    request();
  });

  /* ───────── Card tilt + spotlight ───────── */
  $$(".tilt").forEach(card => {
    card.addEventListener("pointermove", e => {
      const r = card.getBoundingClientRect();
      const x = (e.clientX - r.left) / r.width, y = (e.clientY - r.top) / r.height;
      card.style.setProperty("--ry", `${(x - 0.5) * 8}deg`);
      card.style.setProperty("--rx", `${(0.5 - y) * 8}deg`);
      card.style.setProperty("--gx", `${x * 100}%`);
      card.style.setProperty("--gy", `${y * 100}%`);
    });
    card.addEventListener("pointerleave", () => { card.style.setProperty("--rx", "0deg"); card.style.setProperty("--ry", "0deg"); });
  });

  /* ───────── Replaying card animations ───────── */
  function replay(el, every) {
    let timer;
    const play = () => { el.classList.remove("play"); void el.offsetWidth; el.classList.add("play"); };
    watch(el, () => { play(); clearInterval(timer); timer = setInterval(play, every); }, () => clearInterval(timer));
  }
  replay($("#lanes"), 6500);
  replay($("#recover"), 6000);
  replay($("#reap"), 6500);

  /* ───────── Context window fill → compaction ───────── */
  {
    const bar = $("#ctxBar"), pct = $("#ctxPct"), state = $("#ctxState");
    let timer, busy = false;
    const kinds = ["", "", "u", "t"];
    const capacity = () => Math.floor(bar.clientWidth / 12);
    function step() {
      if (busy) return;
      const bars = [...bar.children];
      const used = bars.reduce((n, b) => n + (b.classList.contains("arc") ? 2.5 : 1), 0);
      const frac = used / capacity();
      pct.textContent = `${Math.round(frac * 100)}%`;
      if (frac >= 0.88) {
        busy = true;
        state.textContent = "compacting…"; state.className = "compact";
        const old = bars.slice(0, Math.max(0, bars.length - 6));
        old.forEach(b => { b.style.height = "6%"; b.style.opacity = ".4"; });
        setTimeout(() => {
          old.forEach(b => b.remove());
          if (!bar.querySelector(".arc")) {
            const arc = document.createElement("i");
            arc.className = "arc"; arc.title = "archived · preview + artifact ref";
            bar.prepend(arc);
          }
          state.textContent = "streaming"; state.className = "";
          busy = false;
        }, 900);
        return;
      }
      const i = document.createElement("i");
      i.className = kinds[Math.floor(Math.random() * kinds.length)];
      i.style.height = `${25 + Math.random() * 70}%`;
      bar.append(i);
    }
    watch($("#ctx"), () => { clearInterval(timer); timer = setInterval(step, 380); }, () => clearInterval(timer));
  }

  /* ───────── Segmented controls ───────── */
  function placePill(seg) {
    const pill = $(".seg-pill", seg), on = $("[aria-selected=true]", seg);
    if (!pill || !on) return;
    pill.style.width = `${on.offsetWidth}px`;
    pill.style.transform = `translateX(${on.offsetLeft}px)`;
  }
  function segment(seg, onPick) {
    $$("button", seg).forEach(b => b.addEventListener("click", () => select(b, true)));
    function select(b, user) {
      $$("button", seg).forEach(x => x.setAttribute("aria-selected", x === b));
      placePill(seg);
      onPick(b, user);
    }
    requestAnimationFrame(() => placePill(seg));
    return select;
  }
  document.fonts && document.fonts.ready.then(() => $$(".seg").forEach(placePill));

  /* ───────── CLI typewriter ───────── */
  {
    const body = $("#cliBody"), tabs = $("#cliTabs");
    const esc = s => s.replace(/&/g, "&amp;").replace(/</g, "&lt;");
    const tool = (k, target, result, dur) => ({ tool: true, html: `<em>●</em> <span class="k">${k}</span>${esc(target)}`, result, dur });
    const scripts = {
      run: {
        cmd: 'minicode run "fix the failing test" --workspace .\\my-app',
        lines: [
          tool("read", "src/parser.rs", '<span class="dim">184 lines</span>', 300),
          tool("grep", '"fn parse_header"', '<span class="dim">3 matches</span>', 350),
          tool("edit", "src/parser.rs", '<span class="ok">+3</span> <span class="bad">−1</span>', 500),
          tool("bash", "cargo test", '<span class="ok">✓ 42 passed</span>', 1300),
          { html: '\n<span class="ok">✔ Done</span> <span class="dur">· 4 tools · 3.1k tokens · 6.2s</span>' },
        ],
      },
      chat: {
        cmd: "minicode chat --workspace .\\my-app",
        lines: [
          { html: '<span class="dim">MiniCode chat · deepseek-v4.1-flash · Ctrl+C to exit</span>\n' },
          { html: '<span class="cmd">you ›</span> where is the retry logic?' },
          tool("grep", '"retry" src/', '<span class="dim">7 matches</span>', 400),
          tool("read", "src/provider.rs", '<span class="dim">L190–240</span>', 400),
          { html: '<em>✻</em> <span class="cmd">provider.rs:212</span> — exponential backoff, 3 attempts,\n  then the error is surfaced to the turn.\n' },
          { html: '<span class="cmd">you ›</span> <span class="cur"></span>' },
        ],
      },
      eval: {
        cmd: "minicode eval --baselines b0,b1,b2 --repeats 3",
        lines: [
          tool("B0", "basic loop        ", '<span class="ok">12/12 ✓</span>', 900),
          tool("B1", "compaction        ", '<span class="ok">12/12 ✓</span>', 900),
          tool("B2", "resume on failure ", '<span class="ok">12/12 ✓</span>', 1100),
          { html: '\n<span class="dim">hidden tests · protected files · clean workspace per attempt</span>' },
          { html: '<span class="ok">✔ Report</span> <span class="dur">→ reports/eval/index.html</span>' },
        ],
      },
    };
    const order = Object.keys(scripts);
    let run = 0, visible = false, idle = true, current = "run", nextTimer;

    async function play(name) {
      const id = ++run;
      clearTimeout(nextTimer);
      const alive = () => id === run;
      const s = scripts[name];
      body.innerHTML = '<span class="ps">PS C:\\projects&gt;</span> <span class="cmd"></span><span class="cur"></span>';
      const cmd = $(".cmd", body), cur = $(".cur", body);
      for (const ch of s.cmd) {
        if (!alive()) return;
        cmd.textContent += ch;
        await sleep(reduce ? 0 : 22 + Math.random() * 40);
      }
      await sleep(350);
      if (!alive()) return;
      cur.remove();
      body.insertAdjacentHTML("beforeend", "\n");
      for (const l of s.lines) {
        if (!alive()) return;
        const line = document.createElement("span");
        line.className = "line";
        line.innerHTML = l.html;
        body.append(line);
        if (l.tool) {
          const spin = document.createElement("span");
          spin.className = "spin"; spin.style.marginLeft = "2ch";
          line.append(spin);
          await sleep(reduce ? 0 : l.dur);
          if (!alive()) return;
          spin.remove();
          line.insertAdjacentHTML("beforeend", `  ${l.result}`);
        } else {
          await sleep(reduce ? 0 : 420);
        }
      }
      if (!alive()) return;
      nextTimer = setTimeout(() => {
        if (!alive()) return;
        if (!visible) { idle = true; return; }
        const next = order[(order.indexOf(name) + 1) % order.length];
        pick($(`[data-script=${next}]`, tabs));
      }, 3200);
    }
    const pick = segment(tabs, b => { current = b.dataset.script; play(current); });
    watch($("#cliTerm"), () => { visible = true; if (idle) { idle = false; play(current); } },
      () => { visible = false; });
  }

  /* ───────── TUI slash commands ───────── */
  {
    const input = $("#tuiInput"), palette = $("#tuiPalette"), feed = $("#tuiFeed"), box = $("#tuiBox");
    const status = $(".tui-status span:nth-child(2)");
    const commands = [
      ["/help", "Show commands", () => "Commands: /model /effort /permissions /resume /skill /continue"],
      ["/model", "Switch model", () => "Model → deepseek-v4.1-pro"],
      ["/effort", "Reasoning effort", () => "Reasoning effort → high"],
      ["/permissions", "Permission mode", () => {
        const modes = ["⏵⏵ accept edits", "⏸ ask every time", "⏵⏵⏵ bypass"];
        status.textContent = modes[(modes.indexOf(status.textContent) + 1) % modes.length];
        return `Permissions → ${status.textContent.replace(/^[⏵⏸]+ /, "")}`;
      }],
      ["/resume", "Resume a session", () => "Resumed “Fix flaky render test” · 14 messages restored"],
      ["/skill", "Run a skill", () => "Skills: review · test · release"],
      ["/continue", "Continue last turn", () => "Continuing from the last checkpoint…"],
    ];
    let matches = [], sel = 0, user = false;

    function render() {
      const v = input.value;
      matches = v.startsWith("/") ? commands.filter(c => c[0].startsWith(v.split(" ")[0])) : [];
      sel = clamp(sel, 0, Math.max(0, matches.length - 1));
      palette.innerHTML = matches.map((c, i) =>
        `<div role="option" class="${i === sel ? "sel" : ""}" data-i="${i}"><b>${c[0]}</b><span>${c[1]}</span></div>`).join("");
      palette.classList.toggle("open", matches.length > 0);
    }
    function push(cls, html) {
      const d = document.createElement("div");
      d.className = `tm ${cls}`; d.innerHTML = html;
      feed.append(d);
      while (feed.children.length > 7) feed.firstElementChild.remove();
    }
    function submit() {
      const v = input.value.trim();
      if (!v) return;
      const cmd = matches[sel];
      input.value = ""; render();
      if (cmd) {
        push("user", `› ${cmd[0]}`);
        setTimeout(() => push("sys", `⎿ ${cmd[2]()}`), 250);
      } else {
        push("user", `› ${v.replace(/&/g, "&amp;").replace(/</g, "&lt;")}`);
        setTimeout(() => push("tool", '<em>●</em> Search <span class="dim">workspace</span>'), 350);
        setTimeout(() => push("text", "This is a demo. <code>minicode tui</code> runs it for real."), 900);
      }
    }
    input.addEventListener("input", () => { sel = 0; render(); });
    input.addEventListener("keydown", e => {
      user = true;
      if (e.key === "ArrowDown" && matches.length) { sel = (sel + 1) % matches.length; render(); e.preventDefault(); }
      else if (e.key === "ArrowUp" && matches.length) { sel = (sel - 1 + matches.length) % matches.length; render(); e.preventDefault(); }
      else if (e.key === "Tab" && matches.length) { input.value = matches[sel][0]; render(); e.preventDefault(); }
      else if (e.key === "Enter") { submit(); e.preventDefault(); }
      else if (e.key === "Escape") { input.value = ""; render(); }
    });
    palette.addEventListener("mousemove", e => {
      const row = e.target.closest("[data-i]");
      if (row && +row.dataset.i !== sel) { sel = +row.dataset.i; render(); }
    });
    palette.addEventListener("click", e => { if (e.target.closest("[data-i]")) { user = true; submit(); input.focus(); } });
    input.addEventListener("focus", () => { user = true; });

    // Self-playing demo until the visitor takes over.
    let demoing = false;
    async function demo() {
      if (demoing || user) return;
      demoing = true;
      for (const word of ["/per", "/res"]) {
        input.value = "";
        for (const ch of word) {
          if (user) break;
          input.value += ch; sel = 0; render();
          await sleep(reduce ? 0 : 160);
        }
        if (user) break;
        await sleep(900);
        if (user) break;
        submit();
        await sleep(1800);
      }
      demoing = false;
    }
    watch(box, demo, null, 0.6);
  }

  /* ───────── Desktop app sequence ───────── */
  {
    const rows = $$("#acGroup [data-i]"), tasks = $$(".ap-task"), answer = $("#acAnswer");
    const fill = $("#apFill"), count = $("#apCount");
    let id = 0;
    const setDone = n => { count.textContent = `${n} / 3`; fill.style.width = `${n / 3 * 100}%`; };
    async function sequence() {
      const me = ++id, alive = () => me === id;
      rows.forEach(r => r.classList.remove("in")); answer.classList.remove("in");
      tasks.forEach(t => t.className = "ap-task"); setDone(0);
      await sleep(400);
      tasks[0].classList.add("run");
      for (let i = 0; i < rows.length; i++) {
        if (!alive()) return;
        rows[i].classList.add("in");
        if (i === 1) { tasks[0].className = "ap-task done"; setDone(1); }
        if (i === 3) tasks[1].classList.add("run");
        await sleep(i === 3 ? 1300 : 650);
      }
      if (!alive()) return;
      tasks[1].className = "ap-task done"; setDone(2); tasks[2].classList.add("run");
      await sleep(700);
      answer.classList.add("in");
      await sleep(1100);
      if (!alive()) return;
      tasks[2].className = "ap-task done"; setDone(3);
      await sleep(4000);
      if (alive() && inView) sequence();
    }
    let inView = false;
    watch(appStage, () => { if (!inView) { inView = true; sequence(); } }, () => { inView = false; id++; }, 0.3);
    segment($("#themeSeg"), b => { app.dataset.theme = b.dataset.theme; });
  }

  /* ───────── Copy clone command ───────── */
  {
    const btn = $("#copyBtn"), code = $("#cloneCmd");
    btn.addEventListener("click", async () => {
      try { await navigator.clipboard.writeText(code.textContent); }
      catch { getSelection().selectAllChildren(code); document.execCommand("copy"); }
      btn.textContent = "Copied ✓"; btn.classList.add("done");
      setTimeout(() => { btn.textContent = "Copy"; btn.classList.remove("done"); }, 1800);
    });
  }
})();
