const names=["周一","周二","周三","周四","周五","周六","周日"];export function weekdayName(day){return names[(day.getUTCDay()+6)%7+1]}
