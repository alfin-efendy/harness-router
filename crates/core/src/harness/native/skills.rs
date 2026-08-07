//! Skills: progressive-disclosure capability docs, mirroring opencode/Claude
//! skills. A skill is a `SKILL.md` (name + description frontmatter, markdown
//! body) under `.agents/skills/<name>/` (project) or
//! `~/.config/ryuzi/skills/<name>/` (global — also where the installer
//! materializes installed skill packs). Only names+descriptions are surfaced
//! to the model up front; the full body is fetched on demand via the `skill`
//! tool.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Which discovery root a skill came from. Project skills are always
/// user-invocable; global skills surface only when bound to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOrigin {
    Project,
    Global,
    /// Shipped inside an installed, ENABLED plugin's `skills/` directory
    /// (Task 9). A live root — never copied into `~/.config/ryuzi/skills` —
    /// so disabling or uninstalling the plugin makes it vanish next
    /// session. Behaves exactly like `Global` for listing/binding purposes:
    /// surfaces in "/" only when bound to the agent, always reachable
    /// through the `skill` tool's index.
    Plugin,
}

/// One discovered skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    /// The leaf directory this skill was discovered in (i.e. the directory
    /// containing its `SKILL.md`), used to resolve companion files the skill
    /// ships alongside its instructions.
    pub dir: PathBuf,
    /// Which discovery root this skill came from.
    pub origin: SkillOrigin,
}

/// The set of available skills.
pub struct SkillRegistry {
    skills: BTreeMap<String, Skill>,
}

impl SkillRegistry {
    /// Discover skills under the project (`.agents/skills`) and global
    /// (`~/.config/ryuzi/skills`) roots.
    pub fn load(work_dir: &Path) -> SkillRegistry {
        Self::load_with(work_dir, &[])
    }

    /// Like [`Self::load`], but also scans `extra` directories — each
    /// can be either:
    ///   - A skills root (i.e. `<extra>/<name>/SKILL.md`), exactly like
    ///     `~/.config/ryuzi/skills`, OR
    ///   - A leaf skill directory (i.e. `<extra>/SKILL.md` directly).
    ///
    /// `extra` dirs are attributed [`SkillOrigin::Global`]. A name already
    /// found in an earlier (project/global) directory wins over one from
    /// `extra`.
    pub fn load_with(work_dir: &Path, extra: &[std::path::PathBuf]) -> SkillRegistry {
        warn_on_legacy_skills_dir(work_dir);
        let mut skills = BTreeMap::new();
        for (base, origin) in skill_dirs(work_dir) {
            merge_skill_root(&mut skills, &base, origin);
        }
        for extra_dir in extra {
            merge_skill_root(&mut skills, extra_dir, SkillOrigin::Global);
        }
        SkillRegistry { skills }
    }

    /// Like [`Self::load`], plus every ENABLED, installed plugin's `skills/`
    /// directory (Task 9) — `plugin_roots` is `(plugin_id,
    /// <install_dir>/skills)` for each, provided by the control plane
    /// (`crate::control::ControlPlane::enabled_plugin_content_roots`).
    ///
    /// Plugin roots are LIVE — never copied into `~/.config/ryuzi/skills` —
    /// scanned fresh on every load with the existing [`read_skills`]/leaf
    /// detection, and attributed [`SkillOrigin::Plugin`]. Precedence order is
    /// Project, then Global, then Plugin — plugin roots are folded in LAST,
    /// so the existing first-wins `entry().or_insert()` collision rule
    /// leaves a same-name project or global skill in place.
    pub fn load_with_plugin_roots(
        work_dir: &Path,
        plugin_roots: &[(String, std::path::PathBuf)],
    ) -> SkillRegistry {
        warn_on_legacy_skills_dir(work_dir);
        let mut skills = BTreeMap::new();
        for (base, origin) in skill_dirs(work_dir) {
            merge_skill_root(&mut skills, &base, origin);
        }
        for (_id, root) in plugin_roots {
            merge_skill_root(&mut skills, root, SkillOrigin::Plugin);
        }
        SkillRegistry { skills }
    }

    /// Skills from the global root only (`~/.config/ryuzi/skills`) — the
    /// no-project catalog path.
    pub fn load_global() -> SkillRegistry {
        let mut skills = BTreeMap::new();
        if let Some(home) = dirs::home_dir() {
            for skill in read_skills(&home.join(".config/ryuzi/skills"), SkillOrigin::Global) {
                skills.entry(skill.name.clone()).or_insert(skill);
            }
        }
        SkillRegistry { skills }
    }

    /// Like [`Self::load_global`], plus every enabled plugin's `skills/`
    /// directory (Task 9) — the no-project catalog path. Plugin roots are
    /// folded in last (first-wins), matching [`Self::load_with_plugin_roots`].
    pub fn load_global_with_plugin_roots(plugin_roots: &[(String, PathBuf)]) -> SkillRegistry {
        let mut skills = BTreeMap::new();
        if let Some(home) = dirs::home_dir() {
            merge_skill_root(
                &mut skills,
                &home.join(".config/ryuzi/skills"),
                SkillOrigin::Global,
            );
        }
        for (_id, root) in plugin_roots {
            merge_skill_root(&mut skills, root, SkillOrigin::Plugin);
        }
        SkillRegistry { skills }
    }

    pub fn get(&self, name: &str) -> Option<Skill> {
        self.skills.get(name).cloned()
    }

    pub fn all(&self) -> Vec<Skill> {
        self.skills.values().cloned().collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    /// A `- name: description` list for the system prompt, or `None` if
    /// empty. Descriptions are truncated to 60 chars — the index is a
    /// scan-and-decide surface, not the skill's full documentation (that's
    /// what the `skill` tool loads on demand).
    pub fn guidance(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let list = self
            .skills
            .values()
            .map(|s| {
                let d: String = s.description.chars().take(60).collect();
                format!("- {}: {d}", s.name)
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "Available skills. You MUST scan this list at the start of every \
             task and load a skill's full instructions with the `skill` tool \
             BEFORE doing work it covers.\n{list}"
        ))
    }
}

fn warn_on_legacy_skills_dir(work_dir: &Path) {
    let legacy = work_dir.join(".ryuzi/skills");
    if legacy.is_dir() {
        tracing::warn!(
            path = %legacy.display(),
            "skills: .ryuzi/skills is no longer scanned; move skills to .agents/skills"
        );
    }
}

/// Merge one discovery root into `skills`, first-wins (`entry().or_insert()`)
/// — an already-present name is kept, so callers control precedence purely
/// by the ORDER they invoke this in. `base` may be either a skills root
/// (`<base>/<name>/SKILL.md`, e.g. `~/.config/ryuzi/skills`) or a leaf skill
/// directory (`<base>/SKILL.md` directly, e.g. a plugin-bundled single-skill
/// bundle) — both shapes are auto-detected exactly as before.
fn merge_skill_root(skills: &mut BTreeMap<String, Skill>, base: &Path, origin: SkillOrigin) {
    if base.join("SKILL.md").is_file() {
        // This is a leaf: parse it as a single skill.
        if let Ok(text) = std::fs::read_to_string(base.join("SKILL.md")) {
            let skill = parse_skill(base, &text, origin);
            skills.entry(skill.name.clone()).or_insert(skill);
        }
    } else {
        // This is a root: scan for subdirectories containing SKILL.md.
        for skill in read_skills(base, origin) {
            let name = skill.name.clone();
            if let Some(kept) = skills.get(&name) {
                // First-wins, but never silently: two plugins shipping the
                // same skill name is invisible otherwise, and unlike commands
                // (which stay reachable at `<plugin-id>/<name>`) the loser
                // here has no fallback route.
                if kept.origin == SkillOrigin::Plugin && origin == SkillOrigin::Plugin {
                    tracing::warn!(
                        skill = %name,
                        "skills: two plugins ship a skill with the same name; keeping the first"
                    );
                }
                continue;
            }
            skills.insert(name, skill);
        }
    }
}

fn skill_dirs(work_dir: &Path) -> Vec<(PathBuf, SkillOrigin)> {
    let mut dirs = vec![(work_dir.join(".agents/skills"), SkillOrigin::Project)];
    if let Some(home) = dirs::home_dir() {
        dirs.push((home.join(".config/ryuzi/skills"), SkillOrigin::Global));
    }
    dirs
}

/// Read `<base>/<name>/SKILL.md` skills from a skills directory.
fn read_skills(base: &Path, origin: SkillOrigin) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return vec![];
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let path = e.path();
            let text = std::fs::read_to_string(path.join("SKILL.md")).ok()?;
            Some(parse_skill(&path, &text, origin))
        })
        .collect()
}

fn parse_skill(dir: &Path, text: &str, origin: SkillOrigin) -> Skill {
    let (frontmatter, body) = super::agents::split_frontmatter_pub(text);
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let mut name = dir_name.clone();
    let mut description = format!("Skill `{dir_name}`");
    for (key, value) in frontmatter {
        match key.as_str() {
            "name" => name = value,
            "description" => description = value,
            _ => {}
        }
    }
    Skill {
        name,
        description,
        body: body.trim().to_string(),
        dir: dir.to_path_buf(),
        origin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_project_skills_from_agents_dir_with_project_origin() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Work with PDFs\n---\nUse pdftotext.",
        )
        .unwrap();
        let reg = SkillRegistry::load(dir.path());
        let s = reg.get("pdf").unwrap();
        assert_eq!(s.origin, SkillOrigin::Project);
    }

    #[test]
    fn legacy_ryuzi_skills_dir_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join(".ryuzi/skills/old");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("SKILL.md"), "---\nname: old\n---\nOld body").unwrap();
        assert!(SkillRegistry::load(dir.path()).get("old").is_none());
    }

    #[test]
    fn discovers_skill_with_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Work with PDFs\n---\nUse pdftotext to extract text.",
        )
        .unwrap();
        let reg = SkillRegistry::load(dir.path());
        let s = reg.get("pdf").unwrap();
        assert_eq!(s.description, "Work with PDFs");
        assert!(s.body.contains("pdftotext"));
        assert_eq!(s.dir, skill_dir);
        assert!(reg.guidance().unwrap().contains("pdf: Work with PDFs"));
    }

    #[test]
    fn load_with_merges_an_extra_dir_alongside_the_worktree_ones() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Work with PDFs\n---\nUse pdftotext to extract text.",
        )
        .unwrap();

        // A plugin-bundled skills root, entirely outside the worktree.
        let extra = tempfile::tempdir().unwrap();
        let extra_skill_dir = extra.path().join("triage");
        std::fs::create_dir_all(&extra_skill_dir).unwrap();
        std::fs::write(
            extra_skill_dir.join("SKILL.md"),
            "---\nname: triage\ndescription: Triage issues\n---\nLabel and assign.",
        )
        .unwrap();

        let reg = SkillRegistry::load_with(dir.path(), &[extra.path().to_path_buf()]);
        assert_eq!(reg.get("pdf").unwrap().description, "Work with PDFs");
        let s = reg.get("triage").unwrap();
        assert_eq!(s.description, "Triage issues");
        assert!(s.body.contains("Label and assign."));
        // Check that both skills are present (there may be other global skills too).
        let names = reg.names();
        assert!(
            names.contains(&"pdf".to_string()),
            "pdf skill must be present"
        );
        assert!(
            names.contains(&"triage".to_string()),
            "triage skill must be present"
        );
    }

    #[test]
    fn load_with_no_extra_dirs_matches_load() {
        let dir = tempfile::tempdir().unwrap();
        // Both load and load_with should have the same result when no extras are
        // provided (but may include global skills from ~/.config/ryuzi/skills, etc).
        let via_load = SkillRegistry::load(dir.path()).names();
        let via_load_with = SkillRegistry::load_with(dir.path(), &[]).names();
        assert_eq!(via_load, via_load_with);
    }

    #[test]
    fn empty_skills_dir_yields_nothing() {
        // read_skills over a non-existent / empty dir returns no skills.
        let dir = tempfile::tempdir().unwrap();
        assert!(read_skills(&dir.path().join(".agents/skills"), SkillOrigin::Project).is_empty());
    }

    #[test]
    fn guidance_truncates_descriptions_to_60_chars_and_demands_a_scan() {
        let mut skills = std::collections::BTreeMap::new();
        skills.insert(
            "x".into(),
            Skill {
                name: "x".into(),
                description: "a".repeat(200),
                body: String::new(),
                dir: std::path::PathBuf::new(),
                origin: SkillOrigin::Project,
            },
        );
        let g = SkillRegistry { skills }.guidance().unwrap();
        assert!(
            g.contains("You MUST scan"),
            "mandatory-scan wording missing: {g}"
        );
        assert!(
            !g.contains(&"a".repeat(61)),
            "description not truncated to 60 chars"
        );
    }

    #[test]
    fn parse_skill_falls_back_to_dir_name() {
        let s = parse_skill(
            Path::new("mytool"),
            "No frontmatter, just a body.",
            SkillOrigin::Project,
        );
        assert_eq!(s.name, "mytool");
        assert!(s.description.contains("mytool"));
        assert_eq!(s.body, "No frontmatter, just a body.");
        assert_eq!(s.dir, Path::new("mytool"));
        assert_eq!(s.origin, SkillOrigin::Project);
    }

    #[test]
    fn load_with_extra_leaf_dir_containing_skill_md() {
        // Test that plugin-bundled leaf skill dirs (SKILL.md directly inside)
        // are discovered as single skills, not roots.
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Work with PDFs\n---\nUse pdftotext to extract text.",
        )
        .unwrap();

        // A plugin-bundled LEAF skill dir (not a root).
        let plugin_dir = tempfile::tempdir().unwrap();
        let plugin_skill = plugin_dir.path().join("github-triage");
        std::fs::create_dir_all(&plugin_skill).unwrap();
        std::fs::write(
            plugin_skill.join("SKILL.md"),
            "---\nname: github-triage\ndescription: Triage GitHub issues\n---\nLabel and assign issues.",
        )
        .unwrap();

        let reg = SkillRegistry::load_with(dir.path(), std::slice::from_ref(&plugin_skill));
        assert_eq!(reg.get("pdf").unwrap().description, "Work with PDFs");
        let s = reg.get("github-triage").unwrap();
        assert_eq!(s.description, "Triage GitHub issues");
        assert!(s.body.contains("Label and assign"));
        assert_eq!(s.dir, plugin_skill);
        // Check that both skills are present (there may be other global skills too).
        let names = reg.names();
        assert!(
            names.contains(&"pdf".to_string()),
            "pdf skill must be present"
        );
        assert!(
            names.contains(&"github-triage".to_string()),
            "github-triage skill must be present"
        );
    }

    #[test]
    fn load_with_extra_root_dir_with_subdirs() {
        // Test that root-shaped extra dirs (with subdirectories containing
        // SKILL.md) still work as before.
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/pdf");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: pdf\ndescription: Work with PDFs\n---\nUse pdftotext to extract text.",
        )
        .unwrap();

        // A plugin-bundled ROOT dir with subdirectories.
        let plugin_root = tempfile::tempdir().unwrap();
        let plugin_skill = plugin_root.path().join("github-triage");
        std::fs::create_dir_all(&plugin_skill).unwrap();
        std::fs::write(
            plugin_skill.join("SKILL.md"),
            "---\nname: github-triage\ndescription: Triage GitHub issues\n---\nLabel and assign issues.",
        )
        .unwrap();

        let reg = SkillRegistry::load_with(dir.path(), &[plugin_root.path().to_path_buf()]);
        assert_eq!(reg.get("pdf").unwrap().description, "Work with PDFs");
        let s = reg.get("github-triage").unwrap();
        assert_eq!(s.description, "Triage GitHub issues");
        assert!(s.body.contains("Label and assign"));
        assert_eq!(s.dir, plugin_skill);
    }

    // ---------- Task 9: plugin-shipped skills ----------

    #[test]
    fn plugin_skill_roots_load_with_plugin_origin_and_lose_collisions() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_skills = tmp.path().join("github/skills");
        std::fs::create_dir_all(plugin_skills.join("gh-fix-ci")).unwrap();
        std::fs::write(
            plugin_skills.join("gh-fix-ci/SKILL.md"),
            "---\nname: gh-fix-ci\ndescription: Debug failing checks\n---\nBody",
        )
        .unwrap();

        let reg = SkillRegistry::load_with_plugin_roots(
            tmp.path(), // work_dir with no .agents/skills
            &[("github".to_string(), plugin_skills.clone())],
        );
        let skill = reg
            .all()
            .into_iter()
            .find(|s| s.name == "gh-fix-ci")
            .unwrap();
        assert_eq!(skill.origin, SkillOrigin::Plugin);
    }

    #[test]
    fn a_same_named_global_skill_beats_a_plugin_skill() {
        let tmp = tempfile::tempdir().unwrap();
        // A plugin skills root with "triage".
        let plugin_skills = tmp.path().join("github/skills");
        std::fs::create_dir_all(plugin_skills.join("triage")).unwrap();
        std::fs::write(
            plugin_skills.join("triage/SKILL.md"),
            "---\nname: triage\ndescription: Plugin triage\n---\nPlugin body",
        )
        .unwrap();

        // A "global" root (simulated via an extra-shaped root passed as the
        // work_dir's own project skills — here we use a real project dir
        // with the SAME name so Project > Plugin precedence is exercised;
        // Project always wins in `skill_dirs`' order regardless.
        let work_dir = tempfile::tempdir().unwrap();
        let project_skill = work_dir.path().join(".agents/skills/triage");
        std::fs::create_dir_all(&project_skill).unwrap();
        std::fs::write(
            project_skill.join("SKILL.md"),
            "---\nname: triage\ndescription: Project triage\n---\nProject body",
        )
        .unwrap();

        let reg = SkillRegistry::load_with_plugin_roots(
            work_dir.path(),
            &[("github".to_string(), plugin_skills)],
        );
        let skill = reg.get("triage").unwrap();
        assert_eq!(skill.origin, SkillOrigin::Project);
        assert_eq!(skill.description, "Project triage");
    }

    #[test]
    fn plugin_roots_do_not_override_each_other_arbitrarily_but_stay_first_wins() {
        // Two plugins shipping the same skill name: first-listed plugin root
        // wins (first-wins `entry().or_insert()`), the second is dropped —
        // documents the existing collision rule rather than adding a new
        // namespacing behavior for skills (unlike commands, Task 9 has no
        // namespacing requirement).
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a/skills/dup");
        let b = tmp.path().join("b/skills/dup");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(
            a.join("SKILL.md"),
            "---\nname: dup\ndescription: From A\n---\nA body",
        )
        .unwrap();
        std::fs::write(
            b.join("SKILL.md"),
            "---\nname: dup\ndescription: From B\n---\nB body",
        )
        .unwrap();

        let reg = SkillRegistry::load_with_plugin_roots(
            tmp.path(),
            &[
                ("a".to_string(), tmp.path().join("a/skills")),
                ("b".to_string(), tmp.path().join("b/skills")),
            ],
        );
        let skill = reg.get("dup").unwrap();
        assert_eq!(skill.origin, SkillOrigin::Plugin);
        assert_eq!(skill.description, "From A");
    }
}
