export interface EnvFileEntry { name: string; path: string; is_dir: boolean; size?: number }
export interface EnvFileListing { path: string; home: string; parent: string | null; entries: EnvFileEntry[]; truncated: boolean }

export function envFileBreadcrumbs(path: string): { label: string; path: string }[] {
  if (!path) return []
  const separator = !path.startsWith('/') && path.includes('\\') ? '\\' : '/'
  const parts = path.split(separator).filter(Boolean)
  const prefix = path.startsWith(separator + separator) ? separator + separator : path.startsWith(separator) ? separator : ''
  const result = prefix ? [{ label: prefix, path: prefix }] : []
  let current = prefix
  for (const part of parts) {
    current += (current && !current.endsWith(separator) ? separator : '') + part
    if (/^[A-Za-z]:$/.test(current)) current += separator
    result.push({ label: part, path: current })
  }
  return result
}

export function filterEnvFileEntries(entries: EnvFileEntry[], search: string): EnvFileEntry[] {
  const needle = search.normalize('NFC').toLocaleLowerCase()
  return entries.filter(entry => entry.name.normalize('NFC').toLocaleLowerCase().includes(needle))
    .sort((a, b) => Number(b.is_dir) - Number(a.is_dir) || a.name.toLocaleLowerCase().localeCompare(b.name.toLocaleLowerCase()))
}

export function selectedEnvFile(listing: EnvFileListing | null, path: string | null, loading: boolean, error: string): string | null {
  if (!listing || !path || loading || error) return null
  return listing.entries.some(entry => entry.path === path && !entry.is_dir) ? path : null
}
