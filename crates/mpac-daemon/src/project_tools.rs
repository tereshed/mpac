use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use toml_edit::{Array, DocumentMut, Item, Table, value};

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Profile {
    pub mcp: BTreeMap<String, Server>,
    pub skills: BTreeMap<String, Skill>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "transport", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Server {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    Http {
        url: String,
    },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Skill {
    description: String,
    instructions: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Harness {
    Claude,
    Codex,
    Agy,
    Hermes,
}

fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.mcp.len() <= 24 && self.skills.len() <= 24,
            "Project tools allow up to 24 servers and 24 skills"
        );
        ensure!(
            self.mcp
                .keys()
                .chain(self.skills.keys())
                .all(|name| valid_name(name)),
            "Use lowercase names with letters, digits and single hyphens"
        );
        for server in self.mcp.values() {
            match server {
                Server::Stdio { command, args } => ensure!(
                    !command.trim().is_empty()
                        && command.len() <= 1024
                        && !command.chars().any(char::is_control)
                        && args.len() <= 32
                        && args
                            .iter()
                            .all(|arg| arg.len() <= 2048 && !arg.chars().any(char::is_control)),
                    "Invalid MCP executable or arguments"
                ),
                Server::Http { url } => {
                    let parsed = url::Url::parse(url).ok().context("Enter an HTTPS or loopback HTTP MCP URL without credentials or query parameters")?;
                    let loopback =
                        matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
                    ensure!(
                        url.len() <= 2048
                            && parsed.host().is_some()
                            && (parsed.scheme() == "https"
                                || (parsed.scheme() == "http" && loopback))
                            && parsed.username().is_empty()
                            && parsed.password().is_none()
                            && parsed.query().is_none()
                            && parsed.fragment().is_none(),
                        "Enter an HTTPS or loopback HTTP MCP URL without credentials or query parameters"
                    );
                }
            }
        }
        for skill in self.skills.values() {
            ensure!(
                !skill.description.trim().is_empty()
                    && skill.description.len() <= 1024
                    && !skill.description.chars().any(char::is_control)
                    && !skill.instructions.trim().is_empty()
                    && skill.instructions.len() <= 24 * 1024
                    && !skill.instructions.contains('\0'),
                "A skill needs a description and Markdown instructions"
            );
        }
        Ok(())
    }

    fn json_servers(&self, harness: Harness) -> Map<String, Value> {
        self.mcp
            .iter()
            .map(|(name, server)| {
                let config = match (harness, server) {
                    (Harness::Claude, Server::Stdio { command, args }) => {
                        json!({ "type":"stdio", "command":command, "args":args })
                    }
                    (_, Server::Stdio { command, args }) => {
                        json!({ "command":command, "args":args })
                    }
                    (Harness::Claude, Server::Http { url }) => json!({ "type":"http", "url":url }),
                    (Harness::Agy, Server::Http { url }) => json!({ "serverUrl":url }),
                    (_, Server::Http { url }) => json!({ "url":url }),
                };
                (name.clone(), config)
            })
            .collect()
    }

    fn codex_servers(&self) -> Table {
        let mut servers = Table::new();
        for (name, server) in &self.mcp {
            let mut entry = Table::new();
            match server {
                Server::Stdio { command, args } => {
                    entry["command"] = value(command.as_str());
                    entry["args"] = value(args.iter().map(String::as_str).collect::<Array>());
                }
                Server::Http { url } => entry["url"] = value(url.as_str()),
            }
            servers[name] = Item::Table(entry);
        }
        servers
    }

    pub fn export(&self, harness: Harness) -> Result<BTreeMap<String, String>> {
        let (config, skills) = match harness {
            Harness::Claude => (".mcp.json", ".claude/skills"),
            Harness::Codex => (".codex/config.toml", ".agents/skills"),
            Harness::Agy => (".agents/mcp_config.json", ".agents/skills"),
            Harness::Hermes => ("config.yaml", "skills"),
        };
        let mut files = BTreeMap::new();
        if !self.mcp.is_empty() {
            let content = match harness {
                Harness::Codex => {
                    let mut doc = DocumentMut::new();
                    doc["mcp_servers"] = Item::Table(self.codex_servers());
                    doc.to_string()
                }
                Harness::Hermes => {
                    // JSON is valid YAML; preserve strings without YAML scalar coercion.
                    serde_json::to_string_pretty(
                        &json!({ "mcp_servers": self.json_servers(harness) }),
                    )? + "\n"
                }
                _ => {
                    serde_json::to_string_pretty(
                        &json!({ "mcpServers": self.json_servers(harness) }),
                    )? + "\n"
                }
            };
            files.insert(config.into(), content);
        }
        for (name, skill) in &self.skills {
            files.insert(
                format!("{skills}/{name}/SKILL.md"),
                format!(
                    "---\nname: {name}\ndescription: {}\n---\n\n{}\n",
                    serde_json::to_string(&skill.description)?,
                    skill.instructions.trim_end()
                ),
            );
        }
        Ok(files)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Save {
    pub expected_revision: u64,
    pub profile: Profile,
}

#[derive(Default, Serialize)]
pub(crate) struct Saved {
    pub revision: u64,
    pub profile: Profile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Apply {
    pub root_id: String,
    pub expected_revision: u64,
}

pub(crate) fn native_exports(profile: &Profile) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for harness in [Harness::Claude, Harness::Codex, Harness::Agy] {
        files.extend(profile.export(harness)?);
    }
    Ok(files)
}

#[cfg(unix)]
fn merge_entries<T: PartialEq>(
    current: &mut BTreeMap<String, T>,
    old: BTreeMap<String, T>,
    next: BTreeMap<String, T>,
    path: &str,
) -> Result<()> {
    let names: std::collections::BTreeSet<_> = old.keys().chain(next.keys()).cloned().collect();
    let mut next = next;
    for name in names {
        let existing = current.get(&name);
        let desired = next.remove(&name);
        if existing == desired.as_ref() {
            continue;
        }
        ensure!(
            existing.is_none() || existing == old.get(&name),
            "Existing MCP entry differs from the MPAC export: {path} ({name})"
        );
        if let Some(entry) = desired {
            current.insert(name, entry);
        } else {
            current.remove(&name);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn merge_config(
    path: &str,
    existing: &str,
    before: Option<&str>,
    desired: Option<&str>,
) -> Result<String> {
    if path.ends_with(".toml") {
        fn servers(doc: &DocumentMut) -> Result<BTreeMap<String, String>> {
            let Some(item) = doc.get("mcp_servers") else {
                return Ok(BTreeMap::new());
            };
            let table = item
                .as_table()
                .context("MCP servers in config.toml must be a table")?;
            Ok(table
                .iter()
                .map(|(key, item)| (key.into(), item.to_string()))
                .collect())
        }
        let parse = |contents: &str| {
            contents
                .parse::<DocumentMut>()
                .ok()
                .context("Invalid existing MCP TOML")
        };
        let mut doc = parse(existing)?;
        let mut current = servers(&doc)?;
        let old = parse(before.unwrap_or_default())?;
        let next = parse(desired.unwrap_or_default())?;
        merge_entries(&mut current, servers(&old)?, servers(&next)?, path)?;
        if doc.get("mcp_servers").is_none() {
            doc["mcp_servers"] = Item::Table(Table::new());
        }
        let table = doc["mcp_servers"]
            .as_table_mut()
            .context("MCP servers in config.toml must be a table")?;
        let removed: Vec<_> = table
            .iter()
            .filter(|(name, _)| !current.contains_key(*name))
            .map(|(name, _)| name.to_owned())
            .collect();
        for name in removed {
            table.remove(&name);
        }
        if let Some(entries) = next.get("mcp_servers").and_then(Item::as_table) {
            for (name, entry) in entries.iter() {
                if !table
                    .get(name)
                    .is_some_and(|item| item.to_string() == entry.to_string())
                {
                    table[name] = entry.clone();
                }
            }
        }
        return Ok(doc.to_string());
    }

    fn object(text: &str) -> Result<Map<String, Value>> {
        serde_json::from_str(text)
            .ok()
            .context("Invalid existing MCP JSON")
    }
    fn servers(map: &Map<String, Value>) -> Result<BTreeMap<String, Value>> {
        let Some(item) = map.get("mcpServers") else {
            return Ok(BTreeMap::new());
        };
        Ok(item
            .as_object()
            .context("mcpServers must be an object")?
            .clone()
            .into_iter()
            .collect())
    }
    let mut config = object(existing)?;
    let mut entries = servers(&config)?;
    merge_entries(
        &mut entries,
        servers(&object(before.unwrap_or("{}"))?)?,
        servers(&object(desired.unwrap_or("{}"))?)?,
        path,
    )?;
    config.insert("mcpServers".into(), serde_json::to_value(entries)?);
    Ok(serde_json::to_string_pretty(&config)? + "\n")
}

/// Apply the explicitly selected export to a worktree. Keep files with
/// unrelated contents intact, and permit retries after individual file writes.
#[cfg(unix)]
pub(crate) fn apply(
    root: &Path,
    old: BTreeMap<String, String>,
    next: BTreeMap<String, String>,
) -> Result<()> {
    use cap_std::fs::{Dir, OpenOptions, OpenOptionsExt as _};
    use std::{
        collections::BTreeSet,
        ffi::OsStr,
        io::{ErrorKind, Read, Write},
        os::unix::fs::OpenOptionsExt as _,
    };

    fn destination(root: &Dir, path: &str) -> Result<Option<(Dir, String)>> {
        let path = Path::new(path);
        let mut dir = root.try_clone()?;
        for component in path.parent().context("Invalid tools path")?.iter() {
            match dir.symlink_metadata(component) {
                Ok(meta) => ensure!(
                    meta.is_dir(),
                    "A project tools folder is not a regular directory"
                ),
                Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            dir = dir.open_dir(component)?;
        }
        let name = path
            .file_name()
            .and_then(OsStr::to_str)
            .context("Invalid tools path")?;
        Ok(Some((dir, name.to_owned())))
    }

    fn read(dir: &Dir, name: &str) -> Result<Option<String>> {
        let metadata = match dir.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            metadata.is_file() && metadata.len() <= 256 * 1024,
            "Project tools target must be a regular file up to 256 KiB"
        );
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut text = String::new();
        dir.open_with(name, &options)?
            .take(256 * 1024 + 1)
            .read_to_string(&mut text)?;
        ensure!(
            text.len() <= 256 * 1024,
            "Project tools target must be a regular file up to 256 KiB"
        );
        Ok(Some(text))
    }

    fn create_parent(root: &Dir, path: &str) -> Result<(Dir, String)> {
        let path = Path::new(path);
        let mut dir = root.try_clone()?;
        for component in path.parent().context("Invalid tools path")?.iter() {
            match dir.create_dir(component) {
                Ok(()) => (),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
            ensure!(
                dir.symlink_metadata(component)?.is_dir(),
                "A project tools folder is not a regular directory"
            );
            dir = dir.open_dir(component)?;
        }
        Ok((
            dir,
            path.file_name()
                .and_then(OsStr::to_str)
                .context("Invalid tools path")?
                .to_owned(),
        ))
    }

    let root = Dir::from_std_file(
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(root)?,
    );
    let paths: BTreeSet<_> = old.keys().chain(next.keys()).collect();
    let mut writes = Vec::new();
    for path in paths {
        let before = old.get(path).map(String::as_str);
        let desired = next.get(path).map(String::as_str);
        let existing = match destination(&root, path)? {
            Some((dir, name)) => read(&dir, &name)?,
            None => None,
        };
        let current = existing.as_deref();
        if current == desired {
            continue;
        }
        let output = if let Some(current) = current.filter(|_| !path.ends_with("/SKILL.md")) {
            if Some(current) == before {
                desired.map(str::to_owned)
            } else {
                Some(merge_config(path, current, before, desired)?)
            }
        } else {
            ensure!(
                current.is_none() || current == before,
                "Existing file differs from the MPAC export: {path}"
            );
            desired.map(str::to_owned)
        };
        if output != existing {
            writes.push((path, existing, output));
        }
    }
    for (path, existing, desired) in writes {
        let (dir, name) = create_parent(&root, path)?;
        ensure!(
            read(&dir, &name)? == existing,
            "Project tools file changed during export: {path}"
        );
        if let Some(content) = desired {
            let tmp = format!(".mpac-{}", uuid::Uuid::new_v4());
            let mut options = OpenOptions::new();
            options.write(true).create_new(true).mode(0o644);
            let result = (|| -> Result<()> {
                let mut file = dir.open_with(&tmp, &options)?;
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                if existing.is_some() {
                    ensure!(
                        read(&dir, &name)? == existing,
                        "Project tools file changed during export: {path}"
                    );
                    dir.rename(&tmp, &dir, &name)?;
                } else {
                    dir.hard_link(&tmp, &dir, &name)?;
                }
                Ok(())
            })();
            let _ = dir.remove_file(&tmp);
            result?;
        } else {
            dir.remove_file(&name)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn apply(
    _root: &Path,
    _old: BTreeMap<String, String>,
    _next: BTreeMap<String, String>,
) -> Result<()> {
    anyhow::bail!("Project tools export is available on macOS and Linux")
}
