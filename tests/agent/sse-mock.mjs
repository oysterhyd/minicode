import http from 'node:http'
import {setTimeout as delay} from 'node:timers/promises'
// Paced local SSE endpoint. Each request takes the next scripted step (the last
// step repeats); a function receives (payload, index) and returns a step.
// Step fields: status, retryAfter, body (non-200 replies); frames (JSON SSE
// events); intervalMs between frames; silentMs before the first frame;
// dropAfter (destroy the socket after N frames); hang (never end); done
// (append data: [DONE]).
export async function sseMock(t,steps){
  const payloads=[],sockets=new Set()
  const pick=(payload,index)=>typeof steps==='function'?steps(payload,index):steps[Math.min(index,steps.length-1)]
  const server=http.createServer((req,res)=>{let body='';req.on('data',d=>body+=d);req.on('end',async()=>{
    const payload=body?JSON.parse(body):null,index=payloads.length;payloads.push(payload);const step=pick(payload,index)||{}
    if((step.status??200)!==200){res.writeHead(step.status,{'content-type':'application/json',...(step.retryAfter!==undefined?{'retry-after':String(step.retryAfter)}:{})});res.end(step.body??'{}');return}
    res.writeHead(200,{'content-type':'text/event-stream'});res.flushHeaders()
    if(step.silentMs)await delay(step.silentMs)
    const frames=step.frames||[]
    for(let i=0;i<frames.length;i++){
      if(res.destroyed)return
      // Let written frames reach the client before the abrupt close.
      if(step.dropAfter!==undefined&&i>=step.dropAfter){await delay(100);res.destroy();return}
      if(i>0&&step.intervalMs)await delay(step.intervalMs)
      res.write(`data: ${JSON.stringify(frames[i])}\n\n`)
    }
    if(step.hang)return
    if(step.done)res.write('data: [DONE]\n\n')
    res.end()
  })})
  server.on('connection',socket=>{sockets.add(socket);socket.on('close',()=>sockets.delete(socket))})
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve))
  t.after(()=>{for(const socket of sockets)socket.destroy();return new Promise(resolve=>server.close(resolve))})
  return {url:`http://127.0.0.1:${server.address().port}`,payloads}
}
// Anthropic stream frames for text split into `chunks` deltas, followed by tool calls.
export function anthropicFrames({text='',chunks=1,tools=[],stop,input=10,output=5}={}){
  const frames=[{type:'message_start',message:{usage:{input_tokens:input}}}];let index=0
  if(text){frames.push({type:'content_block_start',index,content_block:{type:'text',text:''}});const size=Math.ceil(text.length/chunks);for(let i=0;i<text.length;i+=size)frames.push({type:'content_block_delta',index,delta:{type:'text_delta',text:text.slice(i,i+size)}});index++}
  for(const call of tools){frames.push({type:'content_block_start',index,content_block:{type:'tool_use',id:call.id,name:call.name,input:{}}},{type:'content_block_delta',index,delta:{type:'input_json_delta',partial_json:JSON.stringify(call.arguments||{})}});index++}
  frames.push({type:'message_delta',delta:{stop_reason:stop||(tools.length?'tool_use':'end_turn')},usage:{output_tokens:output}},{type:'message_stop'})
  return frames
}
export function openaiFrames({text='',chunks=1}={}){
  const frames=[],size=Math.max(1,Math.ceil(text.length/chunks))
  for(let i=0;i<text.length;i+=size)frames.push({choices:[{delta:{content:text.slice(i,i+size)},finish_reason:null}]})
  frames.push({choices:[{delta:{},finish_reason:'stop'}]})
  return frames
}
// Registers a local service for the mock and returns its model id.
export async function configureService(f,url,style='anthropic',capabilities={}){
  const config=await f.request('saveService',{service:{name:'Local mock',baseUrl:style==='openai'?url+'/v1':url,apiStyle:style,apiKey:'local-mock-key',enabled:true,models:[{modelId:'mock-model',name:'Mock',contextWindow:200000,maxOutputTokens:8192,supportsEffort:false,capabilities}]}})
  return config.defaultModel
}
