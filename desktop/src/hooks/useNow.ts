import { useEffect, useState } from 'react'

/** Re-render on an interval while `active`; returns the current time. */
export function useNow(active: boolean, interval = 1000) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (!active) return
    setNow(Date.now())
    const timer = window.setInterval(() => setNow(Date.now()), interval)
    return () => clearInterval(timer)
  }, [active, interval])
  return now
}
