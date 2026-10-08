async (page) => {
  await page.setViewportSize({width:1500,height:940});
  await page.emulateMedia({reducedMotion:'reduce'});
  await page.reload();
  await page.getByRole('textbox',{name:'任务输入'}).waitFor();
  const output={metric:'event dispatch to first animation-frame boundary after the changed content commits; excludes paint and animation completion',benchmarks:{}};
  const measure=async (name, value) => page.evaluate(({name,value})=>new Promise((resolve,reject)=>{
    const selectors={work_tab:'[role="tab"][id="work-tab-files"]',settings_open:'button[aria-label^="设置"]',context_open:'button[aria-label="上下文窗口详情"]'};
    const present=()=> name==='work_tab' ? document.querySelector('.work-files') : name==='settings_open' ? document.querySelector('[role="dialog"][aria-label="设置"]') : name==='context_open' ? document.querySelector('#context-breakdown') : document.querySelector('.feed-content')?.textContent.includes(value);
    const timer=setTimeout(()=>{observer.disconnect();reject(Error('UI commit timeout: '+name));},5000);
    const observer=new MutationObserver(()=>{if(present()){observer.disconnect();clearTimeout(timer);requestAnimationFrame(()=>resolve(performance.now()-start));}});
    observer.observe(document.body,{subtree:true,childList:true,attributes:true,characterData:true});
    const start=performance.now();
    if(name==='stream_delta') {
      const key=window.__requests.findLast(r=>r.method==='getSession')?.params.clientKey;
      if(!key){reject(Error('missing conversation key'));return;}
      window.__emit({event:'text_delta',clientKey:key,sessionId:'audit',text:value});
    } else document.querySelector(selectors[name]).click();
  }),{name,value});
  for(const name of ['work_tab','settings_open','context_open','stream_delta']) {
    const values=[];
    if(name==='stream_delta') {
      await page.getByRole('button',{name:'审查工作区的未提交改动 10 分钟',exact:true}).first().click();
      await page.getByRole('log',{name:'对话记录'}).waitFor();
    }
    for(let i=-2;i<7;i++) {
      if(name==='work_tab'){await page.getByRole('tab',{name:'改动 3',exact:true}).click();await page.locator('.work-list').waitFor();}
      if(name==='settings_open'){await page.keyboard.press('Escape');}
      if(name==='context_open'){await page.keyboard.press('Escape');}
      const value='New streamed text '+i;
      const elapsed=await measure(name,value);
      if(i>=0)values.push(elapsed);
    }
    values.sort((a,b)=>a-b);
    output.benchmarks[name]={median_ms:values[3],min_ms:values[0],max_ms:values[6],repeats:7};
    await page.keyboard.press('Escape');
  }
  return output;
}
