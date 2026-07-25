//! Slash commands for the native runtime.
//!
//! A command is a named prompt template. Built-ins are `/init` (write an
//! AGENTS.md) and `/review` (review the working changes). Custom commands are
//! discovered from markdown files in `.ryuzi/commands/` (project) and
//! `~/.config/ryuzi/commands/` (global). Templates interpolate `$ARGUMENTS`
//! (all args) and `$1`..`$9` (positional).

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Where a command is offered in "/" autocomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSurfaces {
    pub home: bool,
    pub session: bool,
}

impl Default for CommandSurfaces {
    fn default() -> Self {
        Self {
            home: true,
            session: true,
        }
    }
}

/// One slash command.
#[derive(Debug, Clone)]
pub struct Command {
    pub name: String,
    pub description: String,
    /// The prompt template with `$ARGUMENTS` / `$1`..`$9` placeholders.
    pub template: String,
    /// Optional agent to run this command under.
    pub agent: Option<String>,
    /// Optional model to use for this command's turn.
    pub model: Option<String>,
    /// Whether this command's turn is a subtask.
    pub subtask: bool,
    /// Which composer surfaces list this command in "/" autocomplete.
    pub surfaces: CommandSurfaces,
    /// Whether the command is meaningful only with a project selected.
    pub requires_project: bool,
}

/// A slash command expanded for a particular input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub prompt: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
}

/// The source of a discovered command file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOrigin {
    Builtin,
    Global,
    Project,
}

impl CommandOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

/// A command file as represented on disk for project command CRUD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCommandInput {
    pub name: String,
    pub description: String,
    pub template: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
}

/// A command file read from a project command directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCommandRead {
    pub name: String,
    pub description: String,
    pub template: String,
    pub agent: Option<String>,
    pub model: Option<String>,
    pub subtask: bool,
    pub revision: String,
}

/// A normalized, validated project command name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCommandName(String);

impl ValidatedCommandName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Errors from safe project command file operations.
#[derive(Debug)]
pub enum CommandFileError {
    InvalidName(String),
    NotFound(String),
    RevisionConflict,
    Io(std::io::Error),
}

impl fmt::Display for CommandFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(message) => write!(f, "invalid command name: {message}"),
            Self::NotFound(name) => write!(f, "project command not found: {name}"),
            Self::RevisionConflict => write!(
                f,
                "project command was modified externally; reload it before saving"
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CommandFileError {}

impl From<std::io::Error> for CommandFileError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl Command {
    /// Expand the template with `args` (a single already-split argument string).
    pub fn expand(&self, args: &str) -> String {
        let positional: Vec<&str> = args.split_whitespace().collect();
        let mut out = self.template.replace("$ARGUMENTS", args.trim());
        for (i, p) in positional.iter().enumerate() {
            out = out.replace(&format!("${}", i + 1), p);
        }
        // Unfilled positionals collapse to empty.
        for i in positional.len()..9 {
            out = out.replace(&format!("${}", i + 1), "");
        }
        out
    }
}

// /compact is intercepted as an ACTION in runner::run_turn before command
// resolution — its asset exists only so UIs list it in autocomplete; its
// (empty) template is never sent to a model.
const BUILTIN_COMMAND_ASSETS: &[(&str, &str)] = &[
    ("init", include_str!("builtin_commands/init.md")),
    ("review", include_str!("builtin_commands/review.md")),
    ("compact", include_str!("builtin_commands/compact.md")),
];

fn builtin_commands() -> Vec<Command> {
    BUILTIN_COMMAND_ASSETS
        .iter()
        .map(|(name, text)| parse_command_markdown(name, text))
        .collect()
}

/// A command source together with its precedence metadata.
#[derive(Debug, Clone)]
pub struct RegisteredCommand {
    pub command: Command,
    pub origin: CommandOrigin,
    /// Whether this source is the command that the runtime executes.
    pub effective: bool,
    /// Whether a project source has a global source with the same name.
    pub shadows_global: bool,
}

/// The set of available slash commands.
pub struct CommandRegistry {
    commands: BTreeMap<String, RegisteredCommand>,
    catalog: Vec<RegisteredCommand>,
}

impl CommandRegistry {
    pub fn load(work_dir: &Path) -> CommandRegistry {
        let global = dirs::home_dir()
            .map(|home| home.join(".config/ryuzi/commands"))
            .unwrap_or_default();
        Self::load_from_dirs(Some(work_dir), &global)
    }

    /// Global + builtin commands only — the no-project catalog path.
    pub fn load_without_project() -> CommandRegistry {
        let global = dirs::home_dir()
            .map(|home| home.join(".config/ryuzi/commands"))
            .unwrap_or_default();
        Self::load_from_dirs(None, &global)
    }

    pub(crate) fn load_from_dirs(work_dir: Option<&Path>, global_dir: &Path) -> CommandRegistry {
        let global_commands = read_command_dir(global_dir);
        let project_commands = work_dir.map(read_project_command_dir).unwrap_or_default();
        let builtin_commands = builtin_commands();
        let mut catalog = Vec::new();

        for command in global_commands {
            catalog.push(RegisteredCommand {
                command,
                origin: CommandOrigin::Global,
                effective: false,
                shadows_global: false,
            });
        }
        for command in project_commands {
            let shadows_global = catalog.iter().any(|entry| {
                entry.command.name == command.name && entry.origin == CommandOrigin::Global
            });
            catalog.push(RegisteredCommand {
                command,
                origin: CommandOrigin::Project,
                effective: false,
                shadows_global,
            });
        }
        for command in builtin_commands {
            catalog.push(RegisteredCommand {
                command,
                origin: CommandOrigin::Builtin,
                effective: false,
                shadows_global: false,
            });
        }
        let commands = catalog
            .iter()
            .cloned()
            .map(|entry| (entry.command.name.clone(), entry))
            .collect::<BTreeMap<_, _>>();
        for entry in &mut catalog {
            entry.effective = commands
                .get(&entry.command.name)
                .is_some_and(|effective| effective.origin == entry.origin);
        }
        let commands = commands
            .into_iter()
            .map(|(name, mut entry)| {
                entry.effective = true;
                (name, entry)
            })
            .collect();
        catalog.sort_by(|left, right| {
            left.command
                .name
                .cmp(&right.command.name)
                .then_with(|| left.origin.as_str().cmp(right.origin.as_str()))
        });

        CommandRegistry { commands, catalog }
    }

    pub fn builtin() -> CommandRegistry {
        let catalog = builtin_commands()
            .into_iter()
            .map(|command| RegisteredCommand {
                command,
                origin: CommandOrigin::Builtin,
                effective: true,
                shadows_global: false,
            })
            .collect::<Vec<_>>();
        let commands = catalog
            .iter()
            .cloned()
            .map(|entry| (entry.command.name.clone(), entry))
            .collect();
        CommandRegistry { commands, catalog }
    }

    pub fn get(&self, name: &str) -> Option<Command> {
        self.commands.get(name).map(|entry| entry.command.clone())
    }

    pub fn names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }

    /// All commands, for UI listing.
    pub fn all(&self) -> Vec<Command> {
        self.commands
            .values()
            .map(|entry| entry.command.clone())
            .collect()
    }

    /// Every discovered command source, including sources shadowed by a
    /// higher-precedence project or built-in command.
    pub fn catalog(&self) -> Vec<RegisteredCommand> {
        self.catalog.clone()
    }

    /// If `input` is a slash command (`/name args...`), return its expanded
    /// prompt and metadata. Otherwise return `None`.
    pub fn resolve(&self, input: &str) -> Option<ResolvedCommand> {
        let trimmed = input.trim_start();
        let rest = trimmed.strip_prefix('/')?;
        // A bare "/" or "/ foo" is not a command.
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((n, a)) => (n, a),
            None => (rest, ""),
        };
        let cmd = self.get(name)?;
        Some(ResolvedCommand {
            prompt: cmd.expand(args),
            agent: cmd.agent,
            model: cmd.model,
            subtask: cmd.subtask,
        })
    }
}

fn read_project_command_dir(work_dir: &Path) -> Vec<Command> {
    let ryuzi_dir = work_dir.join(".ryuzi");
    if ryuzi_dir
        .symlink_metadata()
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Vec::new();
    }

    let commands_dir = ryuzi_dir.join("commands");
    if commands_dir
        .symlink_metadata()
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_dir())
    {
        return Vec::new();
    }
    read_command_dir(&commands_dir)
}

fn read_command_dir(dir: &Path) -> Vec<Command> {
    let mut commands = Vec::new();
    read_command_dir_recursive(dir, dir, &mut commands);
    commands
}

fn read_command_dir_recursive(root: &Path, dir: &Path, commands: &mut Vec<Command>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            read_command_dir_recursive(root, &path, commands);
            continue;
        }
        if !file_type.is_file() || path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let Some(name) = command_name_from_path(root, &path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        commands.push(parse_command_markdown(&name, &text));
    }
}

fn command_name_from_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let without_extension = relative.with_extension("");
    let name = without_extension
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?
        .join("/");
    (!name.is_empty()).then_some(name)
}

/// Validate and normalize a project command name before it is resolved to a path.
pub fn validate_project_command_name(name: &str) -> Result<ValidatedCommandName, CommandFileError> {
    let name = validate_project_command_path_name(name)?;
    if is_builtin_command_name(name.as_str()) {
        return Err(CommandFileError::InvalidName(
            "built-in commands cannot be created or updated".into(),
        ));
    }
    Ok(name)
}

fn validate_project_command_path_name(
    name: &str,
) -> Result<ValidatedCommandName, CommandFileError> {
    if name.is_empty() || name.len() > 80 {
        return Err(CommandFileError::InvalidName(
            "must contain 1 through 80 bytes".into(),
        ));
    }
    if name.starts_with('/')
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'/')
        })
    {
        return Err(CommandFileError::InvalidName(
            "only lowercase letters, digits, '-', '_', and '/' are allowed".into(),
        ));
    }
    if name
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(CommandFileError::InvalidName(
            "path segments must not be empty, '.' or '..'".into(),
        ));
    }
    Ok(ValidatedCommandName(name.to_string()))
}

fn is_builtin_command_name(name: &str) -> bool {
    BUILTIN_COMMAND_ASSETS
        .iter()
        .any(|(builtin, _)| *builtin == name)
}

/// The root directory for global commands (`~/.config/ryuzi/commands`),
/// resolved and canonicalized. `create` controls whether the directory is
/// created if missing; when `false` and the directory does not exist, `None`
/// is returned (an empty catalog, not an error).
fn global_command_root(create: bool) -> Result<Option<PathBuf>, CommandFileError> {
    let Some(home) = dirs::home_dir() else {
        return Ok(None);
    };
    let dir = home.join(".config/ryuzi/commands");
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(CommandFileError::InvalidName(
                    "global commands directory must be a real directory".into(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !create {
                return Ok(None);
            }
            std::fs::create_dir_all(&dir)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(Some(dir.canonicalize()?))
}

/// List every readable global command file, including its content revision.
pub fn list_global_commands() -> Result<Vec<ProjectCommandRead>, CommandFileError> {
    match global_command_root(false)? {
        Some(root) => list_commands_at(&root),
        None => Ok(Vec::new()),
    }
}

/// Read one global command by its validated name.
pub fn read_global_command(name: &str) -> Result<ProjectCommandRead, CommandFileError> {
    let validated = validate_project_command_path_name(name)?;
    let Some(root) = global_command_root(false)? else {
        return Err(CommandFileError::NotFound(validated.as_str().to_string()));
    };
    read_project_command_at(&root, &validated)
}

/// Atomically create or update a global command file.
pub fn write_global_command(
    input: ProjectCommandInput,
    expected_revision: Option<&str>,
) -> Result<ProjectCommandRead, CommandFileError> {
    let root = global_command_root(true)?.expect("global command root was created");
    write_command_at(&root, input, expected_revision)
}

/// Delete a global command only when its current content revision matches.
pub fn delete_global_command(name: &str, expected_revision: &str) -> Result<(), CommandFileError> {
    let validated = validate_project_command_path_name(name)?;
    let Some(root) = global_command_root(false)? else {
        return Err(CommandFileError::NotFound(validated.as_str().to_string()));
    };
    delete_command_at(&root, &validated, expected_revision)
}

/// List every readable command file under an arbitrary, already-resolved
/// command root.
fn list_commands_at(root: &Path) -> Result<Vec<ProjectCommandRead>, CommandFileError> {
    let mut commands = Vec::new();
    list_project_commands_recursive(root, root, &mut commands)?;
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(commands)
}

/// Atomically create or update a command file under an arbitrary,
/// already-resolved command root.
fn write_command_at(
    root: &Path,
    input: ProjectCommandInput,
    expected_revision: Option<&str>,
) -> Result<ProjectCommandRead, CommandFileError> {
    let name = validate_project_command_path_name(&input.name)?;
    if is_builtin_command_name(name.as_str()) && expected_revision.is_none() {
        return Err(CommandFileError::InvalidName(
            "built-in commands cannot be created or updated".into(),
        ));
    }
    let mut lock = command_root_lock(root)?;
    let _guard = lock.write()?;
    verify_locked_command_root(root)?;
    let path = project_command_path(root, &name)?;
    if !path.exists() && is_builtin_command_name(name.as_str()) {
        return Err(CommandFileError::InvalidName(
            "built-in commands cannot be created or updated".into(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        reject_symlink_path(root, parent)?;
    }
    if path.exists() {
        let current = read_project_command_at(root, &name)?;
        if expected_revision != Some(current.revision.as_str()) {
            return Err(CommandFileError::RevisionConflict);
        }
    } else if expected_revision.is_some() {
        return Err(CommandFileError::RevisionConflict);
    }

    let content = render_project_command(&input);
    let parent = path.parent().expect("command path has a parent");
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(content.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(&path)
        .map_err(|error| CommandFileError::Io(error.error))?;
    read_project_command_at(root, &name)
}

/// Delete a command file under an arbitrary, already-resolved command root,
/// only when its current content revision matches.
fn delete_command_at(
    root: &Path,
    name: &ValidatedCommandName,
    expected_revision: &str,
) -> Result<(), CommandFileError> {
    let mut lock = command_root_lock(root)?;
    let _guard = lock.write()?;
    verify_locked_command_root(root)?;
    let current = read_project_command_at(root, name)?;
    if current.revision != expected_revision {
        return Err(CommandFileError::RevisionConflict);
    }
    let path = project_command_path(root, name)?;
    std::fs::remove_file(&path)?;
    remove_empty_command_parents(root, path.parent());
    Ok(())
}

fn command_root_lock(root: &Path) -> Result<fd_lock::RwLock<File>, CommandFileError> {
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".commands.lock"))?;
    Ok(fd_lock::RwLock::new(file))
}

/// Re-derive `root`'s canonical path fresh (live, following any current
/// symlinks in every component) and confirm it still matches. This guards
/// against a race where an ancestor directory was swapped for a symlink
/// between the pre-lock path resolution and lock acquisition — a generic
/// check that works for any command root, project or global.
fn verify_locked_command_root(root: &Path) -> Result<(), CommandFileError> {
    if root.canonicalize()?.as_path() != root {
        return Err(CommandFileError::InvalidName(
            "commands directory changed while acquiring its lock".into(),
        ));
    }
    Ok(())
}

fn project_command_path(
    root: &Path,
    name: &ValidatedCommandName,
) -> Result<PathBuf, CommandFileError> {
    let path = root.join(name.as_str()).with_extension("md");
    let parent = path.parent().expect("command path has a parent");
    reject_symlink_path(root, parent)?;
    if !path.starts_with(root) {
        return Err(CommandFileError::InvalidName(
            "command path escaped commands directory".into(),
        ));
    }
    if path.exists() && std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
        return Err(CommandFileError::InvalidName(
            "command file must not be a symlink".into(),
        ));
    }
    Ok(path)
}

fn reject_symlink_path(root: &Path, path: &Path) -> Result<(), CommandFileError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CommandFileError::InvalidName("command path escaped commands directory".into())
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(CommandFileError::InvalidName("invalid command path".into()));
        };
        current.push(component);
        if current.exists()
            && std::fs::symlink_metadata(&current)?
                .file_type()
                .is_symlink()
        {
            return Err(CommandFileError::InvalidName(
                "command paths must not traverse symlinks".into(),
            ));
        }
    }
    Ok(())
}

fn list_project_commands_recursive(
    root: &Path,
    dir: &Path,
    commands: &mut Vec<ProjectCommandRead>,
) -> Result<(), CommandFileError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            list_project_commands_recursive(root, &path, commands)?;
        } else if file_type.is_file() && path.extension().is_some_and(|extension| extension == "md")
        {
            if let Some(name) = command_name_from_path(root, &path) {
                if let Ok(name) = validate_project_command_path_name(&name) {
                    commands.push(read_project_command_at(root, &name)?);
                }
            }
        }
    }
    Ok(())
}

fn read_project_command_at(
    root: &Path,
    name: &ValidatedCommandName,
) -> Result<ProjectCommandRead, CommandFileError> {
    let path = project_command_path(root, name)?;
    if !path.exists() {
        return Err(CommandFileError::NotFound(name.0.clone()));
    }
    let bytes = std::fs::read(&path)?;
    let text = String::from_utf8_lossy(&bytes);
    let command = parse_command_markdown(name.as_str(), &text);
    Ok(ProjectCommandRead {
        name: name.0.clone(),
        description: command.description,
        template: command.template,
        agent: command.agent,
        model: command.model,
        subtask: command.subtask,
        revision: revision(&bytes),
    })
}

fn render_project_command(input: &ProjectCommandInput) -> String {
    let mut frontmatter = format!("---\ndescription: {}\n", input.description);
    if let Some(agent) = input.agent.as_deref() {
        frontmatter.push_str(&format!("agent: {agent}\n"));
    }
    if let Some(model) = input.model.as_deref() {
        frontmatter.push_str(&format!("model: {model}\n"));
    }
    frontmatter.push_str(&format!(
        "subtask: {}\n---\n{}",
        input.subtask, input.template
    ));
    frontmatter
}

fn revision(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn remove_empty_command_parents(root: &Path, mut directory: Option<&Path>) {
    while let Some(dir) = directory {
        if dir == root
            || std::fs::read_dir(dir)
                .ok()
                .and_then(|mut entries| entries.next())
                .is_some()
        {
            break;
        }
        if std::fs::remove_dir(dir).is_err() {
            break;
        }
        directory = dir.parent();
    }
}

fn parse_command_markdown(name: &str, text: &str) -> Command {
    let (frontmatter, body) = super::agents::split_frontmatter_pub(text);
    let mut description = format!("Custom command `/{name}`");
    let mut agent = None;
    let mut model = None;
    let mut subtask = false;
    let mut surfaces = CommandSurfaces::default();
    let mut requires_project = false;
    for (key, value) in frontmatter {
        match key.as_str() {
            "description" => description = value,
            "agent" => agent = Some(value),
            "model" => model = Some(value),
            "subtask" => subtask = matches!(value.trim(), "true" | "TRUE" | "True"),
            "surfaces" => {
                let (mut home, mut session) = (false, false);
                for part in value.split(',').map(str::trim) {
                    match part {
                        "home" => home = true,
                        "session" => session = true,
                        _ => {}
                    }
                }
                if home || session {
                    surfaces = CommandSurfaces { home, session };
                }
            }
            "requires-project" => {
                requires_project = matches!(value.trim(), "true" | "TRUE" | "True")
            }
            _ => {}
        }
    }
    Command {
        name: name.to_string(),
        description,
        template: body,
        agent,
        model,
        subtask,
        surfaces,
        requires_project,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_present() {
        let reg = CommandRegistry::builtin();
        let init = reg.get("init").unwrap();
        assert!(init.template.contains("Analyze this codebase"));
        assert!(init.surfaces.home && init.surfaces.session);
        assert!(init.requires_project);
        let review = reg.get("review").unwrap();
        assert_eq!(review.agent.as_deref(), Some("plan"));
        assert!(!review.surfaces.home && review.surfaces.session);
        let compact = reg.get("compact").unwrap();
        assert!(compact.template.is_empty());
        assert!(!compact.surfaces.home && compact.surfaces.session);
        // Verify templates don't have trailing newlines (byte-identical to originals)
        assert!(!init.template.ends_with('\n'));
        assert!(!review.template.ends_with('\n'));
    }

    #[test]
    fn parses_surfaces_and_requires_project_frontmatter() {
        let cmd = parse_command_markdown(
            "ship",
            "---\ndescription: Ship\nsurfaces: session\nrequires-project: true\n---\nShip it",
        );
        assert!(!cmd.surfaces.home && cmd.surfaces.session);
        assert!(cmd.requires_project);
        // Absent keys default to both surfaces, no project requirement.
        let plain = parse_command_markdown("x", "---\ndescription: X\n---\nBody");
        assert!(plain.surfaces.home && plain.surfaces.session);
        assert!(!plain.requires_project);
    }

    #[test]
    fn resolve_expands_arguments() {
        let reg = CommandRegistry::builtin();
        let resolved = reg.resolve("/review the auth module").unwrap();
        assert!(resolved.prompt.contains("the auth module"));
        assert_eq!(resolved.agent.as_deref(), Some("plan"));
    }

    #[test]
    fn resolve_returns_none_for_plain_text() {
        let reg = CommandRegistry::builtin();
        assert!(reg.resolve("just a normal prompt").is_none());
        assert!(reg.resolve("/unknown-command x").is_none());
    }

    #[test]
    fn expand_fills_positional_and_arguments() {
        let cmd = Command {
            name: "greet".into(),
            description: "d".into(),
            template: "Hello $1, welcome to $2. All: $ARGUMENTS".into(),
            agent: None,
            model: None,
            subtask: false,
            surfaces: CommandSurfaces::default(),
            requires_project: false,
        };
        assert_eq!(
            cmd.expand("Alice Wonderland"),
            "Hello Alice, welcome to Wonderland. All: Alice Wonderland"
        );
        // Unfilled positionals collapse.
        assert_eq!(cmd.expand("Bob"), "Hello Bob, welcome to . All: Bob");
    }

    #[test]
    fn discovers_custom_command() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".ryuzi/commands")).unwrap();
        std::fs::write(
            dir.path().join(".ryuzi/commands/ship.md"),
            "---\ndescription: Ship it\n---\nRun the release checklist. $ARGUMENTS",
        )
        .unwrap();
        let reg = CommandRegistry::load(dir.path());
        let resolved = reg.resolve("/ship now").unwrap();
        assert!(resolved.prompt.contains("release checklist"));
        assert!(resolved.prompt.contains("now"));
    }

    #[cfg(unix)]
    #[test]
    fn skips_project_commands_when_commands_root_or_ryuzi_ancestor_is_a_symlink() {
        use std::os::unix::fs::symlink;

        for symlinked_ancestor in [false, true] {
            let work_dir = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let source = if symlinked_ancestor {
                outside.path().join("commands")
            } else {
                outside.path().to_path_buf()
            };
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(source.join("outside.md"), "outside command").unwrap();

            if symlinked_ancestor {
                symlink(outside.path(), work_dir.path().join(".ryuzi")).unwrap();
            } else {
                std::fs::create_dir(work_dir.path().join(".ryuzi")).unwrap();
                symlink(&source, work_dir.path().join(".ryuzi/commands")).unwrap();
            }

            let registry = CommandRegistry::load(work_dir.path());
            assert!(
                registry.get("outside").is_none(),
                "must not discover a command outside the project through a symlinked root"
            );
        }
    }

    #[test]
    fn reads_nested_command_and_optional_model_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".ryuzi/commands/review/security.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "---\ndescription: Secure\nagent: plan\nmodel: openai/gpt-4.1\nsubtask: true\n---\nReview $ARGUMENTS",
        )
        .unwrap();

        let cmd = CommandRegistry::load(dir.path())
            .get("review/security")
            .unwrap();
        assert_eq!(cmd.description, "Secure");
        assert_eq!(cmd.agent.as_deref(), Some("plan"));
        assert_eq!(cmd.model.as_deref(), Some("openai/gpt-4.1"));
        assert!(cmd.subtask);
    }

    #[test]
    fn catalog_keeps_every_source_while_runtime_precedence_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".ryuzi/commands")).unwrap();
        std::fs::write(dir.path().join(".ryuzi/commands/init.md"), "project init").unwrap();
        std::fs::write(dir.path().join(".ryuzi/commands/ship.md"), "project ship").unwrap();
        std::fs::write(global.path().join("init.md"), "global init").unwrap();
        std::fs::write(global.path().join("ship.md"), "global ship").unwrap();

        let registry = CommandRegistry::load_from_dirs(Some(dir.path()), global.path());
        assert!(registry
            .get("init")
            .unwrap()
            .template
            .contains("Analyze this codebase"));
        assert_eq!(registry.get("ship").unwrap().template, "project ship");

        let sources = registry.catalog();
        let source = |name: &str, origin| {
            sources
                .iter()
                .find(|entry| entry.command.name == name && entry.origin == origin)
                .unwrap()
        };

        assert!(source("ship", CommandOrigin::Project).effective);
        assert!(source("ship", CommandOrigin::Project).shadows_global);
        assert!(!source("ship", CommandOrigin::Global).effective);
        assert!(!source("init", CommandOrigin::Global).effective);
        assert!(!source("init", CommandOrigin::Project).effective);
        assert!(source("init", CommandOrigin::Builtin).effective);
    }

    #[test]
    fn validates_command_names_and_rejects_path_escapes() {
        for name in [
            "",
            "/ship",
            "ship//now",
            "ship/./now",
            "ship/../now",
            "UPPER",
            "init",
        ] {
            assert!(validate_project_command_name(name).is_err(), "{name}");
        }
        assert!(validate_project_command_name("review/security-2_ok").is_ok());
    }

    #[test]
    fn existing_builtin_named_command_is_listed_and_mutable_but_cannot_be_created() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("commands");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.md"), "external project init").unwrap();
        let root = root.canonicalize().unwrap();

        let existing = list_commands_at(&root).unwrap();
        assert_eq!(
            existing
                .iter()
                .map(|command| command.name.as_str())
                .collect::<Vec<_>>(),
            ["init"]
        );

        let updated = write_command_at(
            &root,
            ProjectCommandInput {
                name: "init".into(),
                description: "External project init".into(),
                template: "Updated init".into(),
                agent: None,
                model: None,
                subtask: false,
            },
            Some(&existing[0].revision),
        )
        .unwrap();
        assert_eq!(updated.template, "Updated init");
        let name = validate_project_command_path_name("init").unwrap();
        delete_command_at(&root, &name, &updated.revision).unwrap();

        let fresh_root = tempfile::tempdir().unwrap();
        let fresh_root = fresh_root.path().canonicalize().unwrap();
        let error = write_command_at(
            &fresh_root,
            ProjectCommandInput {
                name: "init".into(),
                description: String::new(),
                template: "New init".into(),
                agent: None,
                model: None,
                subtask: false,
            },
            None,
        )
        .unwrap_err();
        assert!(matches!(error, CommandFileError::InvalidName(_)));
        assert!(
            !fresh_root.join("init.md").exists(),
            "rejecting a new reserved command must not create a command file"
        );
    }

    #[test]
    fn writes_atomically_and_rejects_stale_revision() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let input = ProjectCommandInput {
            name: "review/security".into(),
            description: "Security review".into(),
            template: "Review $ARGUMENTS".into(),
            agent: Some("plan".into()),
            model: Some("openai/gpt-4.1".into()),
            subtask: true,
        };
        let created = write_command_at(&root, input.clone(), None).unwrap();
        assert_eq!(created.revision.len(), 64);
        assert_eq!(created.name, "review/security");

        let error = write_command_at(
            &root,
            ProjectCommandInput {
                template: "changed".into(),
                ..input
            },
            Some("stale"),
        )
        .unwrap_err();
        assert!(matches!(error, CommandFileError::RevisionConflict));
    }

    #[test]
    fn global_style_crud_operates_on_an_arbitrary_root() {
        let root_dir = tempfile::tempdir().unwrap();
        let root = root_dir.path().canonicalize().unwrap();
        let input = ProjectCommandInput {
            name: "ship".into(),
            description: "Ship".into(),
            template: "Ship $ARGUMENTS".into(),
            agent: None,
            model: None,
            subtask: false,
        };
        let created = write_command_at(&root, input.clone(), None).unwrap();
        assert_eq!(created.revision.len(), 64);
        let listed = list_commands_at(&root).unwrap();
        assert_eq!(listed.len(), 1);
        let stale = write_command_at(
            &root,
            ProjectCommandInput {
                template: "changed".into(),
                ..input
            },
            Some("stale"),
        );
        assert!(matches!(
            stale.unwrap_err(),
            CommandFileError::RevisionConflict
        ));
        let name = validate_project_command_name("ship").unwrap();
        delete_command_at(&root, &name, &created.revision).unwrap();
        assert!(list_commands_at(&root).unwrap().is_empty());
    }

    #[test]
    fn command_root_lock_excludes_a_concurrent_mutator_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".ryuzi/commands");
        std::fs::create_dir_all(&root).unwrap();

        let mut held = command_root_lock(&root).unwrap();
        let guard = held.try_write().unwrap();
        let mut contender = command_root_lock(&root).unwrap();
        assert!(
            contender.try_write().is_err(),
            "a mutation must wait for the root lock before checking a revision"
        );
        drop(guard);
        assert!(
            contender.try_write().is_ok(),
            "the mutation lock must be released when the prior mutation finishes"
        );
    }

    #[test]
    fn legacy_command_files_default_new_metadata() {
        let command =
            parse_command_markdown("ship", "---\ndescription: Ship\nagent: plan\n---\nShip");
        assert_eq!(command.model, None);
        assert!(!command.subtask);
    }

    #[test]
    fn resolve_keeps_agent_model_and_subtask_overrides() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".ryuzi/commands")).unwrap();
        std::fs::write(
            dir.path().join(".ryuzi/commands/ship.md"),
            "---\nagent: plan\nmodel: openai/gpt-4.1\nsubtask: true\n---\nShip $ARGUMENTS",
        )
        .unwrap();
        let resolved = CommandRegistry::load(dir.path())
            .resolve("/ship today")
            .unwrap();
        assert_eq!(resolved.prompt, "Ship today");
        assert_eq!(resolved.agent.as_deref(), Some("plan"));
        assert_eq!(resolved.model.as_deref(), Some("openai/gpt-4.1"));
        assert!(resolved.subtask);
    }
}
