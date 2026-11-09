const open = document.querySelector('#open')
const copy = document.querySelector('#copy')
const status = document.querySelector('#status')
let target = 'mpac://open'
try {
  const entries = [...new URLSearchParams(location.hash.slice(1))]
  if (entries.length) {
    if (entries.length !== 1 || entries[0][0] !== 'link' || entries[0][1].length > 8192) throw Error()
    const candidate = new URL(entries[0][1])
    if (candidate.protocol !== 'mpac:' || candidate.hostname !== 'pair' || !['','/'].includes(candidate.pathname) || candidate.username || candidate.password || candidate.port) throw Error()
    const fields = [...candidate.searchParams]
    if (fields.length !== 2 || candidate.searchParams.getAll('v').length !== 1 || candidate.searchParams.get('v') !== '1' || candidate.searchParams.getAll('endpoint').length !== 1) throw Error()
    const computer = new URL(candidate.searchParams.get('endpoint'))
    if (computer.protocol !== 'https:' || computer.username || computer.password || !computer.port || computer.pathname !== '/' || computer.search || computer.hash) throw Error()
    const ip = computer.hostname.split('.').map(Number)
    if (ip.length !== 4 || ip.some(part => !Number.isInteger(part) || part < 0 || part > 255) || !(ip[0] === 10 || ip[0] === 127 || ip[0] === 192 && ip[1] === 168 || ip[0] === 172 && ip[1] >= 16 && ip[1] <= 31 || ip[0] === 169 && ip[1] === 254)) throw Error()
    const trust = new URLSearchParams(candidate.hash.slice(1))
    if ([...trust].length !== 2 || trust.getAll('ca').length !== 1 || !/^[A-Za-z0-9_-]{1,5462}$/.test(trust.get('ca') || '') || trust.getAll('invite').length !== 1 || !/^[a-f0-9]{64}$/.test(trust.get('invite') || '')) throw Error()
    target = candidate.href
    document.querySelector('#description').textContent = 'Review this connection in MPAC, then compare the six-digit code on your computer. Opening this page does not grant access.'
    const label = document.querySelector('#computer'); label.textContent = computer.origin; label.hidden = false
  }
  open.href = target
  copy.addEventListener('click', async () => {
    try { await navigator.clipboard.writeText(target); status.textContent = 'App link copied. Paste it into MPAC if the app does not open.' }
    catch { status.textContent = 'Clipboard is unavailable. Open this page in your browser and try again.' }
  })
} catch {
  open.removeAttribute('href'); open.setAttribute('aria-disabled','true'); copy.disabled = true
  status.textContent = 'Invalid MPAC link. Create a new pairing link on your computer.'
}
// No fetch, analytics, external assets, redirects, or automatic app opening.
// Reused browser tabs must not keep an earlier invitation after a new fragment.
window.addEventListener('hashchange', () => location.reload())
