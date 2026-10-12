export function drain(jobs){const order=[];while(jobs.length)order.push(jobs.pop());return order}
