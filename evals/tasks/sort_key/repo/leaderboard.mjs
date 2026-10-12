export function topScores(players){return [...players].sort((a,b)=>a.score-b.score).map(p=>p.name)}
