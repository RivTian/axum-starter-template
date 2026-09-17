//! 模板侧审计工具：渲染面、泄漏、切片反漂移。
//!
//! 三个子命令：
//! - `template <repo>`：生成前的模板树审计（顶层白名单、垃圾文件、符号链接、ignore 路径、
//!   占位符合法性、`.liquid` 名单）。`ignore` 静默跳过不存在的路径，所以这里必须替它报错。
//! - `generated <dir>`：对**生成结果**做内容级审计（残留占位符、模板内部材料、里程碑词、
//!   模板门禁目标名）。
//! - `slices <repo>`：README 的代码块与 `scripts/slices/` 的答案文件必须逐字一致（反漂移）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("template") => args.get(1).map_or_else(
            || Err("用法：audit template <模板仓库根>".to_owned()),
            |root| audit_template(Path::new(root)),
        ),
        Some("generated") => args.get(1).map_or_else(
            || Err("用法：audit generated <生成结果目录>".to_owned()),
            |dir| audit_generated(Path::new(dir)),
        ),
        Some("slices") => args.get(1).map_or_else(
            || Err("用法：audit slices <模板仓库根>".to_owned()),
            |root| audit_slices(Path::new(root)),
        ),
        _ => Err("用法：audit <template|generated|slices> <路径>".to_owned()),
    };
    match result {
        Ok(checks) => {
            for check in checks {
                println!("  ok  {check}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("audit failed: {message}");
            ExitCode::FAILURE
        }
    }
}

// ── 模板树审计 ──────────────────────────────────────────────────────────────────

/// 模板树根允许出现的条目（其余一律红：残留物、编辑器目录、构建产物都在这里被挡住）。
const TOP_LEVEL_ALLOW: &[&str] = &[
    ".git",
    ".gitignore",
    ".gitignore.liquid",
    "Cargo.toml",
    "Makefile",
    "Makefile.liquid",
    "README.md",
    "README.md.liquid",
    "cargo-generate.toml",
    "crates",
    "docs",
    "hooks",
    "scripts",
    "tools",
];

/// 会被生成结果带出去的顶层目录（`ignore` 之外的都在审计范围内）。
const SHIPPED_DIRS: &[&str] = &["crates", "hooks"];

/// `.liquid` 改名机制只允许这三个文件（生成侧同名文件遮蔽模板侧）。
const EXPECTED_LIQUID: &[&str] = &[".gitignore.liquid", "Makefile.liquid", "README.md.liquid"];

/// 生成契约里必须真的被用到的占位符（少一个说明渲染面被改坏了）。
const REQUIRED_PLACEHOLDERS: &[&str] = &[
    "crate_name",
    "crate_prefix",
    "crate_prefix_snake",
    "env_prefix",
    "project-name",
];

/// cargo-generate 0.24 在渲染面遇到这些序列会直接失败；模板侧提前红。
const LIQUID_TAGS: &[&str] = &["{{", "{%", "{#"];

const JUNK: &[&str] = &[
    ".DS_Store",
    ".cache",
    "target",
    ".idea",
    ".vscode",
    ".claude",
];

fn audit_template(root: &Path) -> Result<Vec<String>, String> {
    let mut checks = Vec::new();

    // 1) 顶层白名单。
    for entry in std::fs::read_dir(root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !TOP_LEVEL_ALLOW.contains(&name.as_str()) {
            return Err(format!(
                "模板树根出现了不在白名单里的 `{name}`：要么删掉，要么加进 tools/audit 的白名单并说明理由"
            ));
        }
    }
    checks.push("顶层白名单".to_owned());

    // 2) 生成面里的垃圾文件与符号链接（cargo-generate 只按字面路径 ignore，递归的残留要靠这里挡）。
    for dir in SHIPPED_DIRS {
        let path = root.join(dir);
        let mut files = Vec::new();
        walk(&path, &mut files);
        for file in &files {
            let name = file
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().to_string());
            if JUNK.contains(&name.as_str()) || name.ends_with(".swp") || name.ends_with(".orig") {
                return Err(format!("生成面里有垃圾文件：`{}`", file.display()));
            }
            if std::fs::symlink_metadata(file)
                .map_err(|err| err.to_string())?
                .file_type()
                .is_symlink()
            {
                return Err(format!(
                    "生成面里有符号链接（cargo-generate 会跳过它）：`{}`",
                    file.display()
                ));
            }
        }
    }
    for file in root_files(root)? {
        let name = file
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().to_string());
        if JUNK.contains(&name.as_str()) {
            return Err(format!("模板树根有垃圾文件：`{}`", file.display()));
        }
    }
    checks.push("生成面无垃圾/无符号链接".to_owned());

    // 3) ignore 里的每条路径都必须真实存在（cargo-generate 对不存在的路径静默跳过）。
    let config_text = std::fs::read_to_string(root.join("cargo-generate.toml"))
        .map_err(|err| format!("读 cargo-generate.toml 失败：{err}"))?;
    let config: toml::Value = toml::from_str(&config_text)
        .map_err(|err| format!("cargo-generate.toml 不是合法 TOML：{err}"))?;
    let ignore = config
        .get("template")
        .and_then(|template| template.get("ignore"))
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    for entry in &ignore {
        let name = entry.as_str().unwrap_or_default();
        if !root.join(name).exists() {
            return Err(format!(
                "cargo-generate.toml 的 ignore 里有不存在的路径 `{name}`：cargo-generate 会静默跳过它（写错了也看不出来）"
            ));
        }
    }
    checks.push(format!("ignore 条目都真实存在（{} 条）", ignore.len()));

    // 3b) 骨架不预建表：迁移目录里不允许出现任何 `.sql`（这是模板纪律，不该变成生成项目里的测试）。
    let migrations = root.join("crates/storage/migrations");
    let mut migration_files = Vec::new();
    walk(&migrations, &mut migration_files);
    let sql: Vec<String> = migration_files
        .iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .map(|path| path.display().to_string())
        .collect();
    if !sql.is_empty() {
        return Err(format!(
            "骨架里不允许预建表：crates/storage/migrations 下出现了 SQL 文件 {sql:?}"
        ));
    }
    checks.push("迁移目录为空（不预建表）".to_owned());

    // 4) `.liquid` 名单。
    let liquid: BTreeSet<String> = root_files(root)?
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| name.ends_with(".liquid"))
        .collect();
    let expected: BTreeSet<String> = EXPECTED_LIQUID
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    if liquid != expected {
        return Err(format!(
            "`.liquid` 名单与预期不一致：实际 {liquid:?}，预期 {expected:?}（改名单请同时改 tools/audit 与文档）"
        ));
    }
    checks.push("`.liquid` 遮蔽名单".to_owned());

    // 5) 占位符：名字必须在白名单里，语法错误提前红，且必需的占位符真的被用到。
    let known = known_placeholders(&config);
    let mut rendered_files = Vec::new();
    collect_render_face(root, &mut rendered_files)?;
    let mut used: BTreeSet<String> = BTreeSet::new();
    for file in &rendered_files {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        for tag in LIQUID_TAGS {
            if text.contains(tag) {
                // `{{` 只可能是占位符；`{%` / `{#` 一律不允许出现在模板里。
                if *tag != "{{" {
                    return Err(format!(
                        "`{}` 里出现了 Liquid 标签 `{tag}`：模板不使用标签语法",
                        file.display()
                    ));
                }
            }
        }
        for name in extract_placeholders(&text) {
            if !known.contains(&name) {
                return Err(format!(
                    "`{}` 里出现了未知占位符 `{{{{{name}}}}}`：已知的有 {known:?}",
                    file.display()
                ));
            }
            used.insert(name);
        }
    }
    for required in REQUIRED_PLACEHOLDERS {
        if !used.contains(*required) {
            return Err(format!(
                "占位符 `{{{{{required}}}}}` 在整个渲染面里没有被使用：渲染面被改坏了，或者改名漏了这一步"
            ));
        }
    }
    checks.push(format!("渲染面占位符合法（用到 {} 个）", used.len()));

    Ok(checks)
}

// ── 生成结果审计 ────────────────────────────────────────────────────────────────

/// 生成结果里不允许出现的内容：模板内部材料、里程碑/阶段词、模板侧门禁目标名。
const FORBIDDEN_IN_GENERATED: &[&str] = &[
    // 模板内部材料
    "cargo-generate",
    "docs/architecture",
    "docs/references",
    "docs/acceptance",
    "docs/verification",
    "docs/prompts",
    // 模板侧门禁的目标名（生成结果只有 make check）
    "make gate",
    "make audit",
    "make matrix",
    "make probe",
    "make slices",
    // 里程碑 / 阶段编号（注意：不能拦"阶段"或"Phase"这种普通词——StopPhase 之类的正常代码会中招）
    "里程碑",
    "milestone",
    "阶段 1",
    "阶段 2",
    "阶段 3",
    "阶段一",
    "阶段二",
    "阶段三",
    "Phase 1",
    "Phase 2",
    "Phase 3",
    "DP1",
    "DP2",
    "DP3",
    "DP4",
    "DP5",
    "DP6",
    "DP7",
    "DP8",
    "DP9",
    "DP10",
];

const REQUIRED_IN_GENERATED: &[&str] = &[
    "Cargo.toml",
    "Makefile",
    "README.md",
    ".gitignore",
    "crates/app/src/main.rs",
    "crates/core/src/lib.rs",
    "crates/config/config-template.toml",
    "crates/storage/migrations/README.md",
];

fn audit_generated(dir: &Path) -> Result<Vec<String>, String> {
    let mut checks = Vec::new();

    for required in REQUIRED_IN_GENERATED {
        if !dir.join(required).exists() {
            return Err(format!("生成结果缺少 `{required}`"));
        }
    }
    for forbidden_path in ["docs", "scripts", "tools", "hooks", "cargo-generate.toml"] {
        if dir.join(forbidden_path).exists() {
            return Err(format!("生成结果里出现了模板内部材料 `{forbidden_path}`"));
        }
    }
    checks.push("必需文件齐全、模板材料不在".to_owned());

    let mut files = Vec::new();
    walk(dir, &mut files);
    for file in &files {
        let name = file
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().to_string());
        if name.ends_with(".liquid") {
            return Err(format!("生成结果里有 `.liquid` 残留：`{}`", file.display()));
        }
        if JUNK.contains(&name.as_str()) {
            return Err(format!("生成结果里有垃圾文件：`{}`", file.display()));
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue; // 二进制文件（理论上没有）跳过
        };
        for tag in LIQUID_TAGS {
            if text.contains(tag) {
                return Err(format!(
                    "生成结果里残留了 Liquid 记号 `{tag}`：`{}`",
                    file.display()
                ));
            }
        }
        for needle in FORBIDDEN_IN_GENERATED {
            if text.contains(needle) {
                return Err(format!(
                    "生成结果 `{}` 里出现了模板内部信息 `{needle}`",
                    file.display()
                ));
            }
        }
    }
    checks.push(format!("{} 个文件无残留/无内部信息", files.len()));

    // 生成侧 Makefile 只能有这三条门禁（外加 check 聚合）。
    let makefile = std::fs::read_to_string(dir.join("Makefile")).map_err(|err| err.to_string())?;
    for target in ["fmt-check:", "lint:", "test:", "check:"] {
        if !makefile.contains(target) {
            return Err(format!("生成侧 Makefile 缺少目标 `{target}`"));
        }
    }
    checks.push("生成侧门禁目标齐全".to_owned());

    Ok(checks)
}

// ── 切片反漂移 ──────────────────────────────────────────────────────────────────

/// 切片代码块在 README 里的标记：`<!-- slice: task/flush.rs -->`，紧跟一个围栏代码块。
const SLICE_MARKER: &str = "<!-- slice:";

/// 反漂移用的固定身份：与 `make slices` 生成项目时用的一致。
const FIXTURE_PREFIX: &str = "svc";
const FIXTURE_NAME: &str = "dev_service";

fn audit_slices(root: &Path) -> Result<Vec<String>, String> {
    let readme = std::fs::read_to_string(root.join("README.md.liquid"))
        .map_err(|err| format!("读 README.md.liquid 失败：{err}"))?;
    let rendered = render_fixture(&readme);

    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut lines = rendered.lines().enumerate();
    while let Some((_, line)) = lines.next() {
        let Some(rest) = line.trim().strip_prefix(SLICE_MARKER) else {
            continue;
        };
        let slice = rest.trim().trim_end_matches("-->").trim().to_owned();
        let (_, fence) = lines
            .next()
            .ok_or_else(|| format!("`{slice}` 的标记后面没有代码块"))?;
        if !fence.trim_start().starts_with("```") {
            return Err(format!("`{slice}` 的标记后面不是围栏代码块：{fence}"));
        }
        let mut body = String::new();
        for (_, line) in lines.by_ref() {
            if line.trim_start().starts_with("```") {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
        blocks.push((slice, body));
    }
    if blocks.is_empty() {
        return Err("README.md.liquid 里没有任何切片代码块".to_owned());
    }

    for (slice, body) in &blocks {
        let path = root.join("scripts/slices").join(slice);
        let expected = std::fs::read_to_string(&path).map_err(|err| {
            format!(
                "切片 `{slice}` 的答案文件读不到（{}）：{err}",
                path.display()
            )
        })?;
        if body.trim_end() != expected.trim_end() {
            return Err(format!(
                "切片 `{slice}` 的 README 代码块与答案文件不一致：改一处必须改另一处"
            ));
        }
    }

    Ok(vec![format!("{} 个切片代码块与答案文件一致", blocks.len())])
}

fn render_fixture(text: &str) -> String {
    text.replace("{{crate_prefix_snake}}", FIXTURE_PREFIX)
        .replace("{{crate_prefix}}", FIXTURE_PREFIX)
        .replace("{{crate_name}}", FIXTURE_NAME)
        .replace("{{project-name}}", "dev-service")
        .replace("{{env_prefix}}", &FIXTURE_NAME.to_uppercase())
}

// ── 工具 ────────────────────────────────────────────────────────────────────────

fn known_placeholders(config: &toml::Value) -> BTreeSet<String> {
    // cargo-generate 内建 + pre hook 派生（派生变量在 hooks/pre.rhai 里 set，下面还有一条断言）。
    let mut known: BTreeSet<String> = [
        "project-name",
        "crate_name",
        "authors",
        "username",
        "os-arch",
        "crate_type",
        "within_cargo_project",
        "is_init",
        "crate_prefix_snake",
        "env_prefix",
    ]
    .iter()
    .map(|name| (*name).to_owned())
    .collect();
    if let Some(placeholders) = config.get("placeholders").and_then(toml::Value::as_table) {
        known.extend(placeholders.keys().cloned());
    }
    known
}

fn extract_placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index + 1 < bytes.len() {
        if &bytes[index..index + 2] == b"{{" {
            let Some(end) = text[index + 2..].find("}}") else {
                break;
            };
            let name = text[index + 2..index + 2 + end].trim().to_owned();
            names.push(name);
            index += 2 + end + 2;
        } else {
            index += 1;
        }
    }
    names
}

/// 渲染面：`ignore` 之外的模板树文件（`docs` / `scripts` / `tools` / `.git` 不参与渲染）。
fn collect_render_face(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    fn recurse(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|err| err.to_string())? {
            let entry = entry.map_err(|err| err.to_string())?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            if path.is_dir() {
                if relative == "docs"
                    || relative == "scripts"
                    || relative == "tools"
                    || relative == ".git"
                {
                    continue;
                }
                recurse(&path, root, out)?;
            } else {
                out.push(path);
            }
        }
        Ok(())
    }
    out.push(root.join("Makefile.liquid"));
    out.push(root.join("README.md.liquid"));
    out.push(root.join(".gitignore.liquid"));
    out.push(root.join("cargo-generate.toml"));
    out.push(root.join("Cargo.toml"));
    for dir in SHIPPED_DIRS {
        recurse(&root.join(dir), root, out)?;
    }
    // 模板侧根文件只在顶层白名单里出现，不参与渲染审计（它们不进生成结果）。
    out.retain(|path| path.exists());
    Ok(())
}

fn root_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|err| err.to_string())? {
        let entry = entry.map_err(|err| err.to_string())?;
        if entry.path().is_file() {
            out.push(entry.path());
        }
    }
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            // 生成结果里 cargo-generate 会 git init：`.git` 与构建产物不属于交付面。
            if name == ".git" || name == "target" {
                continue;
            }
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_placeholders_without_touching_single_braces() {
        let names = extract_placeholders("a {{crate_prefix}}-core and {not_a_placeholder} done");
        assert_eq!(names, vec!["crate_prefix"]);
    }

    #[test]
    fn render_fixture_substitutes_all_known_names() {
        let rendered = render_fixture(
            "{{crate_prefix_snake}}_app {{crate_name}} {{project-name}} {{env_prefix}}",
        );
        assert_eq!(rendered, "svc_app dev_service dev-service DEV_SERVICE");
    }
}
