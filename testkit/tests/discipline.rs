//! 结构纪律的自动化证据。
//!
//! 这个文件里的每一条用例，管的都是**编译器永远不会告诉你的事**：把 `-testkit` 从
//! `[dev-dependencies]` 挪进 `[dependencies]` 能编译、能通过全部测试，然后夹具就进了
//! 二进制；在 `select!` 里漏写 `biased;` 能编译，然后关停时偶尔会多跑一轮；给一个带
//! 智能构造器的 newtype 加一行 `Deserialize` 能编译，然后配置文件那条路径绕过了全部
//! 不变量。这类缺陷的共同点是**没有运行期信号**——它们不会 panic、不会报错、不会让任何
//! 现有用例变红，只会在某一天以一个看起来毫不相干的症状出现。
//!
//! ## 为什么是 Rust 测试，不是 Makefile 里的 shell
//!
//! 门禁的唯一入口是 `make check`（三条：`fmt` / `lint` / `test`），而这些检查落在 `test`
//! 这一条里。另一条路是在 `Makefile` 里写 20 段 `grep` / `awk`，代价有三样：
//!
//!   1. 平台差异。BSD grep 没有 `-P`，BSD sed 没有 `-i ''` 之外的写法，macOS 的 make 是
//!      3.81（没有 `.ONESHELL`）。生成结果会在别人的机器上跑，而"门禁在我这儿是绿的"
//!      不是一个可以接受的结论。
//!   2. 依赖面。写进 shell 就等于承认门禁需要一套 Unix 工具链；写在这里，生成结果的门禁
//!      依赖**只有 Rust 工具链本身**，这句话才是真的。
//!   3. 失败消息。集合比对失败时人要看的是"多了什么、少了什么"，`diff <(...) <(...)`
//!      给不出这个，`assert_eq!` 给得出。
//!
//! ## 为什么住在 `testkit`
//!
//! 它得住在某个成员里，而 `testkit` 是唯一一个**定义上就不进二进制**的成员（下面有一条
//! 用例盯着这件事）。放在 `app` 会让最上层多背一件与装配无关的事；放在 `core` 会让叶子层
//! 反过来读上层的源码。这个文件不 `use` 夹具里的任何东西，它只读仓库。
//!
//! ## 它读的是源码文本，不是 AST
//!
//! 每一条扫描都建立在三条**已核对过的文件形状**上（第一条用例就在验这三条）：全仓库没有
//! `/* */` 块注释、`mod tests` 永远在第 0 列、单元测试永远是文件的最后一项。形状变了，
//! 第一条用例先红——而不是让后面十几条悄悄扫了个寂寞。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// `unused_crate_dependencies` 逐 target 判定，而普通依赖会被链进集成测试目标。这个文件
// 一个夹具都不用，所以四条依赖全要在这里交代一声。`as _` 只满足链接检查，不引入任何
// 可命名的东西——另一条路是编几个用不上的断言把它们用起来，那是往门禁里塞假证据。
use service_core as _;
use service_testkit as _;
use tempfile as _;
use tracing as _;
use tracing_subscriber as _;

/// 六个成员的目录名。这些是**目录**，不是包名——包名带项目前缀，目录名不带，所以这个
/// 文件里一个模板变量都不需要（`.rs` 生成前后逐字节相同）。
const MEMBERS: [&str; 6] = ["core", "storage", "worker", "api", "testkit", "app"];

// ═══════════════════════════════════════════════════════════════════════════
// 0. 扫描赖以成立的前提
// ═══════════════════════════════════════════════════════════════════════════

/// 后面每一条扫描都先把注释和单元测试去掉，而那个去法建立在三条形状约定上。约定破了
/// 而这条用例不在的话，后果不是某条扫描变红，是它们**静默地少扫了一大片**——那比没有
/// 门禁更糟，因为报告上是绿的。
#[test]
fn the_source_tree_keeps_the_shape_every_other_scan_assumes() {
    let sources = all_sources("src").expect("read src trees");
    let mut problems = Vec::new();

    for src in &sources {
        for (no, text) in &src.lines {
            // ① 没有块注释。有的话 `//` 逐行剥离就漏了。
            if text.contains("/*") {
                problems.push(format!(
                    "{}:{no}: block comment — the comment stripper only handles `//`",
                    src.path
                ));
            }
            // ② `mod tests` 永远在第 0 列。缩进的话"从这里剥到文件尾"就剥错了位置。
            if text.trim_start().starts_with("mod tests") && !text.starts_with("mod tests") {
                problems.push(format!(
                    "{}:{no}: indented `mod tests` — the stripper cuts at column 0 only",
                    src.path
                ));
            }
        }
        // ③ `mod tests` 之后没有别的项。有的话那些项会被连带剥掉。
        if let Some(cut) = src
            .lines
            .iter()
            .position(|(_, t)| t.starts_with("mod tests"))
        {
            let tail_items = src.lines[cut + 1..]
                .iter()
                .filter(|(_, t)| starts_an_item(t))
                .count();
            if tail_items > 0 {
                problems.push(format!(
                    "{}: {tail_items} top-level item(s) after `mod tests` — they would be \
                     stripped away with the tests",
                    src.path
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. 公共出口与 README 两侧集合相等
// ═══════════════════════════════════════════════════════════════════════════

/// 每个 crate 的 README 里有一份"这个 crate 对外露了什么"的清单，这条用例让它与源码
/// **互为对方的门禁**：加一个 `pub` 不改 README 是红的，改了 README 不加 `pub` 也是红的。
///
/// 有 feature 的 crate **按开关分别比对，不取并集**。取并集的话，把一个本该门控的名字
/// 漏进默认出口就查不出来——而那正是「存储单后端最小编译闭包」要守住的那条线。
#[test]
fn every_public_export_is_listed_in_the_crate_readme() {
    let mut problems = Vec::new();
    let mut seen = 0;

    for member in MEMBERS {
        let readme = read(&format!("{member}/README.md")).expect("read crate README");
        let features = declared_features(&readme);

        for feature in &features {
            let listed = readme_exports(&readme, feature);
            let actual = public_surface(member, feature).expect("walk the module tree");

            let scope = if feature == "default" {
                format!("{member} (default features)")
            } else {
                format!("{member} (--features {feature})")
            };

            diff_sets(
                &mut problems,
                &scope,
                "export",
                &listed.items,
                &actual.items,
            );
            diff_sets(
                &mut problems,
                &scope,
                "public module",
                &listed.modules,
                &actual.modules,
            );
            seen += actual.items.len() + actual.modules.len();
        }
    }

    // 两个空集合也是相等的。标记写错、`lib.rs` 改名、`pub mod` 的形态变了——任何一种都
    // 会让这条用例在什么都没比的情况下变绿。
    assert_saw("public exports", seen, 80);
    assert!(problems.is_empty(), "{}", report(&problems));
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. 清单纪律
// ═══════════════════════════════════════════════════════════════════════════

/// 成员之间的依赖边**等于**分层邻接表，不是包含于它。
///
/// 相等比较是重点。"包含"检查（每条边都在表里）放得过**新增**边：加一条
/// `worker → storage` 之后图仍然是无环的、仍然编译、测试仍然全绿，而任务面从此直接
/// 抓着存储实现——门面那层就白立了。多一条少一条都得红。
///
/// 判据读的是**清单文本**，不是 `cargo metadata` 的已解析图——后者要派生进程，而这里不
/// 派生。差别要说清楚：清单文本看不见 feature 打开后多出来的边。这条守的是"有人手工加
/// 了一行 `[dependencies]`"，而那正是使用者这边实际会发生的那种越界。
#[test]
fn the_dependency_edges_equal_the_layering_table() {
    // 分层邻接表，逐行抄过来。`core` 是叶子，`app` 是汇点。
    const BUILD_EDGES: [(&str, &[&str]); 6] = [
        ("core", &[]),
        ("storage", &["service-core"]),
        ("worker", &["service-core"]),
        ("api", &["service-core"]),
        (
            "app",
            &[
                "service-core",
                "service-storage",
                "service-worker",
                "service-api",
            ],
        ),
        ("testkit", &["service-core"]),
    ];

    let mut problems = Vec::new();

    for (member, expected) in BUILD_EDGES {
        let manifest = read(&format!("{member}/Cargo.toml")).expect("read member manifest");
        let mut build: Vec<String> = Vec::new();
        let mut dev: Vec<String> = Vec::new();

        for (_, section, key, _) in dependency_entries(&manifest, &mut problems, member) {
            if !key.starts_with("service-") {
                continue;
            }
            match section.as_str() {
                "dependencies" => build.push(key),
                "dev-dependencies" => dev.push(key),
                _ => problems.push(format!(
                    "{member}/Cargo.toml: `{key}` in [{section}] — internal members belong in \
                     [dependencies] or [dev-dependencies], nowhere else"
                )),
            }
        }

        // 两边都排序再比：表里的顺序是**分层顺序**（core 在前、api 在后），清单里的顺序
        // 是作者写下的顺序，两者没有理由一致，比的是集合。
        build.sort();
        let mut want: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
        want.sort();
        if build != want {
            problems.push(format!(
                "{member}: build edges are {build:?}, the layering table says {want:?} — \
                 adding or removing one needs a written reason in the table first"
            ));
        }

        // dev 边只允许一条：`→ testkit`。这是表里唯一的额外边，也只该是那一条。
        for edge in &dev {
            if edge != "service-testkit" {
                problems.push(format!(
                    "{member}/Cargo.toml: dev edge `{edge}` — the only extra edge the table \
                     allows is `→ testkit`"
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

/// `testkit` 只能是 dev 依赖。
///
/// 这条比邻接表门禁更窄也更关键：邻接表管**方向**，它管**依赖的种类**。把 dev 依赖误写
/// 成常规依赖，方向没变、编译照过、测试全绿，而夹具（连同它的 `tracing-subscriber`）
/// 进了发布二进制。
#[test]
fn the_test_fixtures_never_become_a_normal_dependency() {
    let mut problems = Vec::new();

    for member in MEMBERS {
        let manifest = read(&format!("{member}/Cargo.toml")).expect("read member manifest");
        for (no, section, key, value) in dependency_entries(&manifest, &mut problems, member) {
            if section != "dependencies" {
                continue;
            }
            if key.ends_with("-testkit") || value.contains("-testkit") {
                problems.push(format!(
                    "{member}/Cargo.toml:{no}: `{key}` is a normal dependency — the fixtures \
                     would be linked into the shipped binary; move it to [dev-dependencies]"
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 成员清单里不得出现裸版本号。
///
/// 版本的唯一定义点是根清单的 `[workspace.dependencies]`，那里每一条都配了一句"为什么
/// 需要它"。成员里写一个 `serde = "1"`，等于在依赖图里开了第二个版本决定点，而两个决定点
/// 迟早会给出两个答案——症状是同一个类型在两处不兼容，报错离原因很远。
#[test]
fn member_manifests_never_decide_a_version_themselves() {
    let mut problems = Vec::new();

    for member in MEMBERS {
        let manifest = read(&format!("{member}/Cargo.toml")).expect("read member manifest");
        for (no, _section, key, value) in dependency_entries(&manifest, &mut problems, member) {
            if !value.contains("workspace = true") {
                problems.push(format!(
                    "{member}/Cargo.toml:{no}: `{key}` does not inherit from the workspace — \
                     versions are decided once, in the root manifest"
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 每个成员都继承 workspace 的 lint 配置。
///
/// 漏掉一个成员的话，那一层就悄悄退出了 `unsafe_code = forbid`、`unwrap_used`、
/// `await_holding_lock` 的管辖——而 `cargo clippy --workspace` 依然是绿的。
#[test]
fn every_member_inherits_the_workspace_lints() {
    let mut problems = Vec::new();

    for member in MEMBERS {
        let manifest = read(&format!("{member}/Cargo.toml")).expect("read member manifest");
        let inherits = manifest
            .lines()
            .skip_while(|l| l.trim() != "[lints]")
            .nth(1)
            .is_some_and(|l| l.trim() == "workspace = true");
        if !inherits {
            problems.push(format!(
                "{member}/Cargo.toml: missing `[lints]` + `workspace = true` — this member \
                 would silently opt out of the workspace lint policy"
            ));
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 整个 workspace 只有一个可执行入口。
///
/// 判据是静态的，不去问 `cargo metadata`（那要派生进程，而生成结果不派生进程）：
/// `[[bin]]` 声明一条，`src/main.rs` 一个，
/// `src/bin/` 一个都没有。后两条比数 `[[bin]]` 更重要：cargo 会**自动发现**它们，
/// 一个谁都没声明的第二入口就是这么来的。
#[test]
fn the_workspace_has_exactly_one_executable_entry_point() {
    let mut declared = Vec::new();
    let mut discovered = Vec::new();

    for member in MEMBERS {
        let manifest = read(&format!("{member}/Cargo.toml")).expect("read member manifest");
        if manifest.lines().any(|l| l.trim() == "[[bin]]") {
            declared.push(member);
        }
        if root().join(member).join("src/main.rs").is_file() {
            discovered.push(format!("{member}/src/main.rs"));
        }
        if root().join(member).join("src/bin").is_dir() {
            discovered.push(format!("{member}/src/bin/"));
        }
    }

    assert_eq!(declared, vec!["app"], "`[[bin]]` declarations");
    assert_eq!(
        discovered,
        vec!["app/src/main.rs".to_owned()],
        "auto-discovered binary targets"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. 源码纪律
// ═══════════════════════════════════════════════════════════════════════════

/// 任务名是枚举，不是可以拿来比较的字符串。
///
/// `if name == "http"` 这种写法在改名之后**照常编译**，而关池那一步会被静默
/// 跳过。枚举让同一个错误变成编译失败。
#[test]
fn task_names_are_never_compared_as_string_literals() {
    let mut problems = Vec::new();
    for src in production().expect("read production sources") {
        for (no, text) in &src.lines {
            if text.contains("== \"http\"") || text.contains("== \"ticker\"") {
                problems.push(format!(
                    "{}:{no}: task name compared as a string literal — use the `TaskName` enum, \
                     so that renaming it fails to compile instead of silently not matching",
                    src.path
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 全进程只有一个路径锚点：可执行文件所在目录。
///
/// `current_dir()` 是一个隐式的环境读取。一旦有第二个锚点，"配置从哪来"就取决于谁在
/// 哪个目录里敲的命令——而那是排障时最先被忽略、最后才被想到的变量。
#[test]
fn nothing_in_the_workspace_reads_the_current_working_directory() {
    let mut problems = Vec::new();
    for src in production().expect("read production sources") {
        for (no, text) in &src.lines {
            if text.contains("current_dir") {
                problems.push(format!(
                    "{}:{no}: `current_dir` — the only path anchor is the executable's own \
                     directory (`core::paths::install_root_from`)",
                    src.path
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 没有人隐式地去取"当前 runtime"（判据：全 workspace 零出现）。
///
/// `Handle::current()` 与 `current_dir()` 同类，都是隐式环境读取：它让"这个任务跑在哪"
/// 在测试里不可控，而多 runtime 正是本模板要能验的东西。装配层收的是 `&Executors`，
/// 句柄由 `RuntimeSet::executors()` 显式给出，一次隐式读取都不需要。
///
/// **唯一豁免**是 `app/src/rt.rs` 里那一处 `Handle::try_current()`：它在 `Drop` 里
/// **探测**"我是不是正被在 async 上下文里 drop"——那是一次条件判断，不是取句柄来用。
#[test]
fn no_layer_reaches_for_the_ambient_runtime_handle() {
    let mut problems = Vec::new();
    let mut probes = Vec::new();

    for src in production().expect("read production sources") {
        for (no, text) in &src.lines {
            if text.contains("Handle::current(") {
                problems.push(format!(
                    "{}:{no}: `Handle::current()` — take the handle from `&Executors` instead",
                    src.path
                ));
            }
            // 豁免按**文件**记，不按行号记：行号会烂，而一条钉在行号上的豁免会在下一次
            // 无关的编辑里变红，然后被人顺手改成一个更宽的形态。
            if text.contains("Handle::try_current(") && !probes.contains(&src.path) {
                probes.push(src.path.clone());
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
    assert_eq!(
        probes,
        vec!["app/src/rt.rs".to_owned()],
        "the only allowed `try_current` is the drop-context probe in `app/src/rt.rs`; \
         a new one needs its own reason written down here"
    );
}

/// 每一个 `select!` 都是取消优先。
///
/// `select!` 默认**随机**选就绪分支。关停时这意味着：取消已经发出，而某个分支恰好也就绪
/// 了，于是任务又多做了一轮。它不会报错，只会让关停偶尔比预期慢一拍——一个复现不了的
/// 现象。`biased;` 把"取消永远排第一"从概率变成事实。
#[test]
fn every_select_puts_cancellation_first() {
    let mut problems = Vec::new();
    let mut seen = 0;
    for src in production().expect("read production sources") {
        for (idx, (no, text)) in src.lines.iter().enumerate() {
            if !text.contains("select! {") {
                continue;
            }
            seen += 1;
            let next = src.lines.get(idx + 1).map(|(_, t)| t.trim());
            if next != Some("biased;") {
                problems.push(format!(
                    "{}:{no}: `select!` without `biased;` as its first line — branch order \
                     would be random, and cancellation has to win",
                    src.path
                ));
            }
        }
    }
    assert_saw("`select!` sites", seen, 6);
    assert!(problems.is_empty(), "{}", report(&problems));
}

/// `allow` 只打在最小单元上。
///
/// 一个模块级的 `#![allow(dead_code)]` 会让整个 `extract` 模块退出死代码
/// 检测。模块级和 crate 级的 `allow` 作用域是**将来会长大的那个东西**，所以它豁免的不止
/// 是写它的人看见的那几行。
///
/// 两类豁免，各自有理由：
///   - `app/src/main.rs` 与 `app/tests/*.rs` 的 `#![allow(unused_crate_dependencies)]`：
///     这条 lint 逐 target 判定，而那几个 target 只用到自家 lib；`app` 声明的其余依赖
///     服务于 lib target，在那边照常被管着。
///   - `api/src/extract.rs` 的三处 `#[allow(clippy::disallowed_types)]`：包装提取器
///     **必须**念出被禁的 `axum::extract::Json` 才能包装它，而那正是禁令的目的地。
#[test]
fn allow_attributes_are_attached_to_the_smallest_unit() {
    const CRATE_LEVEL_EXEMPT: [&str; 3] = [
        "app/src/main.rs",
        "app/tests/run.rs",
        "app/tests/bad_config.rs",
    ];

    let mut problems = Vec::new();
    let mut sources = all_sources("src").expect("read src trees");
    sources.extend(all_sources("tests").expect("read tests trees"));

    for src in &sources {
        for (idx, (no, text)) in src.lines.iter().enumerate() {
            let trimmed = text.trim_start();
            if trimmed.starts_with("#![allow(") && !CRATE_LEVEL_EXEMPT.contains(&src.path.as_str())
            {
                problems.push(format!(
                    "{}:{no}: crate-level `allow` — its scope is everything this file will \
                     ever contain; attach it to the item instead",
                    src.path
                ));
            }
            if trimmed.starts_with("#[allow(") {
                let attached_to = src.lines[idx + 1..]
                    .iter()
                    .map(|(_, t)| t.trim_start())
                    .find(|t| !t.starts_with('#') && !t.is_empty());
                if attached_to.is_some_and(|t| t.starts_with("mod ") || t.starts_with("pub mod ")) {
                    problems.push(format!(
                        "{}:{no}: `allow` attached to a `mod` declaration — same scope problem \
                         as a crate-level one",
                        src.path
                    ));
                }
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 错误类型的 `#[from]` 只跨 workspace 内部的边界。
///
/// `#[from]` 会让 `?` 隐式地做一次类型转换。对第三方错误用它，等于把"这个外部失败该
/// 归成本层的哪一类"这个判断从代码里删掉——于是 `sqlx::Error` 的十几种情况会被压成一个
/// 笼统的变体，而调用方再也分不出"库文件不存在"和"SQL 写错了"。
#[test]
fn error_conversions_only_cross_workspace_boundaries() {
    const LOCAL: [&str; 8] = [
        "crate",
        "self",
        "super",
        "service_core",
        "service_storage",
        "service_worker",
        "service_api",
        "service_testkit",
    ];

    let mut problems = Vec::new();
    let mut seen = 0;
    for src in production().expect("read production sources") {
        for (idx, (no, text)) in src.lines.iter().enumerate() {
            if !text.contains("#[from]") {
                continue;
            }
            seen += 1;
            // `#[from]` 挂在字段上：`Config(#[from] service_core::config::ConfigError)`,
            // 或者独占一行、类型在下一行。两种都要看到类型。
            let after = text.split("#[from]").nth(1).unwrap_or("").trim().to_owned();
            let ty = if after.is_empty() {
                src.lines
                    .get(idx + 1)
                    .map(|(_, t)| t.trim().to_owned())
                    .unwrap_or_default()
            } else {
                after
            };
            let head = ty
                .trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .split("::")
                .next()
                .unwrap_or("")
                .trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_')
                .to_owned();

            if !ty.contains("::") {
                continue; // 同模块内的类型，本来就在 workspace 里
            }
            if !LOCAL.contains(&head.as_str()) {
                problems.push(format!(
                    "{}:{no}: `#[from]` on a third-party error (`{head}`) — classify it \
                     explicitly instead, so that the caller keeps the distinctions the \
                     upstream type makes",
                    src.path
                ));
            }
        }
    }
    assert_saw("`#[from]` attributes", seen, 3);
    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 带智能构造器的 newtype 不得 `#[derive(Deserialize)]`。
///
/// `derive` 直接构造内部字段，**完全绕过**构造器里的不变量——而且是静默的。失效的恰好是
/// "从配置文件加载"这条唯一真实使用的路径：手写代码走构造器（验了），配置文件走 derive
/// （没验），于是不变量在最重要的入口处不成立。正解是手写 `Deserialize`，先反序列化成
/// 一个朴素类型再进构造器。
#[test]
fn newtypes_with_smart_constructors_do_not_derive_deserialize() {
    let mut problems = Vec::new();

    for src in production().expect("read production sources") {
        let derives = derived_types(&src);
        for (idx, (_, text)) in src.lines.iter().enumerate() {
            let Some(ty) = text.strip_prefix("impl ").and_then(|r| r.split(' ').next()) else {
                continue;
            };
            let Some(&derive_line) = derives.get(ty) else {
                continue;
            };
            // 在这个 impl 块里找一个返回 Result / Option 的 `fn new(`。
            for (no, body) in src.lines[idx + 1..].iter() {
                if body == "}" {
                    break;
                }
                if body.contains("fn new(")
                    && (body.contains("-> Result") || body.contains("-> Option"))
                {
                    problems.push(format!(
                        "{}:{no}: `{ty}` has a fallible `new` but also derives Deserialize \
                         (line {derive_line}) — deserialization would bypass the invariant \
                         on the one path that actually matters",
                        src.path
                    ));
                }
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. 进程边界
// ═══════════════════════════════════════════════════════════════════════════

/// 进程边界适配器里不得有判断。
///
/// 生成结果不派生进程，所以这两个文件是全仓库**唯一**没有自动化证据的地方。
/// "没有证据"只有在它们不含决策时才等于"没有风险"——一旦它们开始做预算计算或优先级
/// 判断，那部分逻辑就既没人测、也没人看。
///
/// 数字是实测出来的，不是拍的。改动它要连同理由一起改：
///   - `boot/env.rs` 0 条：它只是把 `current_exe()` 的结果转交出去。
///   - `signals.rs` 4 条：两条 `match self`（枚举转字符串）、一条 `for` 遍历信号源、
///     一条 `match poll_recv`。都是单层转发。
#[test]
fn the_process_boundary_adapters_contain_no_decisions() {
    const BUDGETS: [(&str, usize); 2] = [("app/src/boot/env.rs", 0), ("app/src/signals.rs", 4)];

    for (path, budget) in BUDGETS {
        let src = strip(&load_source(path).expect("read adapter"));
        let found = branch_count(&src);
        assert!(
            found <= budget,
            "{path}: {found} branches, budget is {budget}. this file has no automated \
             evidence behind it (the generated project spawns no processes), so it may \
             only forward -- if the new branch is genuinely unavoidable, raise the budget \
             here and write down why."
        );
    }
}

/// `main()` 不承载任何判断。
///
/// 整个进程的可执行逻辑必须是一个**可调用的函数**（`app::run`），因为 `main()` 里的逻辑
/// 天然不可测：没有办法从一个用例里调用它、给它喂参数、读它的返回值。
#[test]
fn main_carries_no_logic_of_its_own() {
    const LINE_BUDGET: usize = 80;

    let raw = load_source("app/src/main.rs").expect("read main.rs");
    let total = raw.lines.len();
    assert!(
        total <= LINE_BUDGET,
        "app/src/main.rs is {total} lines (budget {LINE_BUDGET}) — everything worth testing \
         belongs in `app::run`"
    );

    let found = branch_count(&strip(&raw));
    assert_eq!(
        found, 0,
        "app/src/main.rs contains {found} branch(es) — a decision made here can never be \
         reached from a test"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. 具体的、没有运行期信号的陷阱
// ═══════════════════════════════════════════════════════════════════════════

/// 每一条结构化事件都有显式的 `name:`，而且都留了一句给人读的话。
///
/// 不写 `name:` 的话 `tracing` 自动生成的名字是 `event api/src/system.rs:90` 这种
/// **带行号**的东西——行号会烂，靠它做断言等于把用例钉在源码布局上。
///
/// 反过来，只有 `name:` 和字段、没有消息的事件，人读日志时看到的是一行键值对，得自己
/// 去源码里查这个名字是什么意思。两个角色都要有：断言读 `name`，人读消息。
#[test]
fn every_event_has_both_a_stable_name_and_a_human_message() {
    const LEVELS: [&str; 5] = ["trace!(", "debug!(", "info!(", "warn!(", "error!("];

    let mut problems = Vec::new();
    let mut seen = 0;
    for src in production().expect("read production sources") {
        for (idx, (no, text)) in src.lines.iter().enumerate() {
            // 词边界不是洁癖：`compile_error!(` 的结尾就是 `error!(`，而 `signals.rs` 里那条
            // 「非 unix 平台请自己实现」的编译期错误正好长这样。不查边界的话，第一次运行就
            // 会指着一条根本不是事件的东西说它缺 `name:`。
            let Some((pos, lvl)) = LEVELS
                .iter()
                .find_map(|lvl| word_start(text, lvl).map(|p| (p, *lvl)))
            else {
                continue;
            };
            let call = join_macro_call(&src, idx, pos + lvl.len() - 1);
            seen += 1;

            let args = call
                .trim_start_matches('(')
                .trim_end_matches(&[')', ';'][..]);
            if !args.trim_start().starts_with("name:") {
                problems.push(format!(
                    "{}:{no}: event without an explicit `name:` — tracing would name it after \
                     this file and line number, and line numbers rot",
                    src.path
                ));
                continue;
            }
            let last = args.trim_end().trim_end_matches(',').trim_end();
            if !last.ends_with('"') {
                problems.push(format!(
                    "{}:{no}: event has `name:` but no human-readable message — the name is \
                     for assertions, the message is for the person reading the log",
                    src.path
                ));
            }
        }
    }

    assert_saw("tracing events", seen, 40);
    assert!(problems.is_empty(), "{}", report(&problems));
}

/// 业务路由树上不得挂 fallback。
///
/// 实测（axum 0.8.9）：`Router::merge` 见到两个 fallback 时走的是 `(true, false)` 臂,
/// **不 panic**——业务的那个会被随后的 `.fallback(not_found)` 静默覆盖。也就是说这个错误
/// 唯一的症状，是使用者精心写的 404 处理**从来不生效**，而没有任何一处会告诉他。
/// 顶层 fallback 是信封纪律的执行点，所以覆盖的方向是对的；错的是它没有声音。
#[test]
fn the_business_router_never_installs_a_fallback() {
    let src = strip(&load_source("api/src/business.rs").expect("read business.rs"));
    let hits: Vec<String> = src
        .lines
        .iter()
        .filter(|(_, t)| t.contains(".fallback"))
        .map(|(no, _)| format!("api/src/business.rs:{no}"))
        .collect();

    assert!(
        hits.is_empty(),
        "{hits:?}: a fallback here is silently overridden by the top-level one when the \
         routers are merged (axum 0.8.9 takes the `(true, false)` branch — no panic, no \
         warning). the envelope discipline owns the fallback; put your 404 handling in a \
         route instead."
    );
}

/// 配置重载不自己读文件（A 续 24/27）。
///
/// 启动和重载读的是同一个文件、同一批失败形态。重载这边再写一份读取逻辑，两份就会漂：
/// 启动时拦得住的坏文件，重载时可能拦不住——而重载失败**不是**进程失败，旧配置还在用，
/// 于是没有人会注意到。复用 `bootstrap` 里那套 `read_limited` / `classify_read` 才能让
/// 两条路径的失败形态天然一致。
#[test]
fn config_reload_does_not_grow_its_own_file_reader() {
    let src = strip(&load_source("app/src/config/reload.rs").expect("read reload.rs"));
    let mut problems = Vec::new();

    for (no, text) in &src.lines {
        for needle in ["std::fs", "fs::", "bootstrap::load"] {
            if text.contains(needle) {
                problems.push(format!(
                    "app/src/config/reload.rs:{no}: `{needle}` — reuse the bootstrap reader \
                     (`read_limited` / `classify_read`) so both paths fail the same way"
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

/// README 不许说"另外还要跑 X"。
///
/// 把最有价值的门禁拆成一个单独的目标，结果就是没有任何东西会自动跑它。一个需要人
/// 记得的门禁不是门禁。`make check` 是全部子目标的并集，README 里不该有第二个入口。
#[test]
fn the_readme_names_make_check_as_the_only_gate() {
    let readme = read("README.md").expect("read the project README");
    let mut problems = Vec::new();

    for (idx, line) in readme.lines().enumerate() {
        for needle in ["另外还要跑", "还需要单独跑", "记得再跑"] {
            if line.contains(needle) {
                problems.push(format!(
                    "README.md:{}: \"{needle}\" — a gate that needs someone to remember it \
                     is not a gate; fold it into `make check`",
                    idx + 1
                ));
            }
        }
    }

    assert!(problems.is_empty(), "{}", report(&problems));
}

// ═══════════════════════════════════════════════════════════════════════════
// 读取与剥离
// ═══════════════════════════════════════════════════════════════════════════

/// 一份源文件：仓库相对路径 + 带原始行号的行。行号是原始的，剥离之后也不变——失败消息
/// 里的位置必须能直接跳到编辑器里。
struct Source {
    path: String,
    lines: Vec<(usize, String)>,
}

/// workspace 根。
///
/// 用 `CARGO_MANIFEST_DIR` 而不是 `current_dir()`：cargo 可以从任何目录被调用，而这些
/// 用例读的是**仓库**。这与"全进程只有一个路径锚点"是同一条理由，只是发生在测试里。
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn read(rel: &str) -> io::Result<String> {
    fs::read_to_string(root().join(rel))
}

fn load_source(rel: &str) -> io::Result<Source> {
    let text = read(rel)?;
    Ok(Source {
        path: rel.to_owned(),
        lines: text
            .lines()
            .enumerate()
            .map(|(i, l)| (i + 1, l.to_owned()))
            .collect(),
    })
}

/// 六个成员在 `<member>/<sub>` 下的全部 `.rs`，不剥离。
fn all_sources(sub: &str) -> io::Result<Vec<Source>> {
    let mut out = Vec::new();
    for member in MEMBERS {
        let dir = root().join(member).join(sub);
        if !dir.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_rs(&dir, &mut files)?;
        files.sort();
        for file in files {
            let rel = file
                .strip_prefix(root())
                .unwrap_or(&file)
                .to_string_lossy()
                .into_owned();
            out.push(load_source(&rel)?);
        }
    }
    Ok(out)
}

/// 生产代码：`<member>/src` 下的全部 `.rs`，去掉注释与单元测试。
fn production() -> io::Result<Vec<Source>> {
    Ok(all_sources("src")?.iter().map(strip).collect())
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_rs(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// 去掉注释与 `mod tests` 之后的一切。
///
/// 这个剥离是后面每一条扫描的地基，它成立靠的是三条文件形状约定，而那三条由
/// `the_source_tree_keeps_the_shape_every_other_scan_assumes` 盯着。不剥的话，第一轮
/// 实测的结果是：任务名字面量一条、`current_dir` 一条、`Handle::current` 十五条、`.fallback` 四条——**全部
/// 来自文档注释和单元测试**。那种门禁只会逼人把判据改松，直到它不再报警为止。
fn strip(src: &Source) -> Source {
    let mut lines = Vec::new();
    for (no, text) in &src.lines {
        if text.starts_with("mod tests") {
            break;
        }
        let code = strip_trailing_comment(text);
        if code.trim().is_empty() && !text.trim().is_empty() {
            continue; // 整行都是注释
        }
        lines.push((*no, code));
    }
    Source {
        path: src.path.clone(),
        lines,
    }
}

/// 去掉行尾注释，但不碰字符串字面量里的 `//`（URL 就长这样）。
fn strip_trailing_comment(text: &str) -> String {
    let bytes: Vec<char> = text.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if escaped {
            escaped = false;
        } else if c == '\\' && in_string {
            escaped = true;
        } else if c == '"' {
            in_string = !in_string;
        } else if c == '/' && !in_string && bytes.get(i + 1) == Some(&'/') {
            return bytes[..i].iter().collect::<String>().trim_end().to_owned();
        }
        i += 1;
    }
    text.to_owned()
}

/// 一行是不是开启了一个顶层项。用来验"`mod tests` 之后没有别的项"。
fn starts_an_item(text: &str) -> bool {
    const HEADS: [&str; 10] = [
        "pub ", "fn ", "struct ", "enum ", "trait ", "impl ", "mod ", "use ", "type ", "const ",
    ];
    HEADS.iter().any(|h| text.starts_with(h))
}

/// `needle` 在 `haystack` 里第一次**以词首出现**的位置。
///
/// 这个词边界是这份文件里最容易被省掉、也最容易出事的一件事。两个实测出来的例子：
/// `compile_error!(` 的尾巴就是 `error!(`，`notify_for(` 的尾巴就是 `for(`。裸 `find`
/// 会把两者都算上，而"门禁第一次运行就指着一条无关的行报红"最常见的下场，不是有人去
/// 修扫描器，是有人把判据改松。
fn word_start(haystack: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let pos = from + rel;
        let boundary = !haystack[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if boundary {
            return Some(pos);
        }
        from = pos + needle.len();
    }
    None
}

/// 语法级分支计数：`if` / `match` / `while` / `for`。
///
/// `for` 有一个必须排掉的假阳性：`impl Stream for OsSignals` 里的 `for` 不是循环。
/// 泛型的 `for<'a>` 同理。不排的话 `signals.rs` 会被算成 6 条而不是 4 条，然后预算就会
/// 被调大到一个不再说明任何事情的数字。
fn branch_count(src: &Source) -> usize {
    let mut n: usize = 0;
    for (_, text) in &src.lines {
        let t = text.trim_start();
        if t.starts_with("impl ") {
            continue;
        }
        for kw in ["if ", "match ", "while ", "for "] {
            let mut rest = t;
            while let Some(pos) = word_start(rest, kw) {
                n += 1;
                rest = &rest[pos + kw.len()..];
            }
        }
        if t.contains("for<") {
            n = n.saturating_sub(1);
        }
    }
    n
}

/// 把一次可能跨行的宏调用拼成一行，从 `(` 开始到配对的 `)` 为止。
///
/// 括号计数要跳过字符串字面量：消息里带括号是常事（`"(see the shutdown plan)"`）,
/// 不跳过的话拼接会在半路停下，然后"最后一个参数是不是字符串"就判错了。
fn join_macro_call(src: &Source, start_line: usize, open_at: usize) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, (_, text)) in src.lines.iter().enumerate().skip(start_line) {
        let slice: String = if idx == start_line {
            text.chars().skip(open_at).collect()
        } else {
            out.push(' ');
            text.trim().to_owned()
        };
        for c in slice.chars() {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' && in_string {
                escaped = true;
            } else if c == '"' {
                in_string = !in_string;
            } else if !in_string && c == '(' {
                depth += 1;
            } else if !in_string && c == ')' {
                depth -= 1;
                if depth == 0 {
                    return out;
                }
            }
        }
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// 公共出口的两侧提取
// ═══════════════════════════════════════════════════════════════════════════

/// 一个 crate 在某个 feature 组合下的出口。
#[derive(Default)]
struct Surface {
    items: BTreeSet<String>,
    modules: BTreeSet<String>,
}

/// README 里声明了哪几份清单。`default` 永远有；`feature=` 标记出来的按序追加。
fn declared_features(readme: &str) -> Vec<String> {
    let mut out = vec!["default".to_owned()];
    for line in readme.lines() {
        if let Some(rest) = line.trim().strip_prefix("<!-- exports:begin")
            && let Some(f) = rest.split("feature=").nth(1)
        {
            let name = f.split_whitespace().next().unwrap_or("").to_owned();
            if !name.is_empty() && name != "default" && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// README 一侧：只取 `exports:begin` / `exports:end` 之间的内容。
///
/// 为什么要显式标记，而不是"扫 `## 公共出口` 这一节里的反引号"：那一节的散文里也有反引号
/// 包着的标识符（`Router`、`AppState::new(...)`），`worker` 的那张表里还有 `TickerPlane`
/// 的**关联项**。让扫描器去猜哪些算数，它迟早会猜错——而猜错的方向通常是"多收了几个"，
/// 于是有人为了让门禁变绿，把一个本不该出现在清单里的名字加进 README。
fn readme_exports(readme: &str, feature: &str) -> Surface {
    let mut out = Surface::default();
    let mut active = false;

    for line in readme.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("<!-- exports:begin") {
            let block = rest
                .split("feature=")
                .nth(1)
                .and_then(|f| f.split_whitespace().next())
                .unwrap_or("default");
            // `test-utils` 那份清单是**在默认清单之上追加**的，所以默认块也算进来。
            active = block == "default" || block == feature;
            continue;
        }
        if trimmed.starts_with("<!-- exports:end") {
            active = false;
            continue;
        }
        if !active {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("### ") {
            for name in backticked(rest) {
                out.modules.insert(name);
            }
        } else if trimmed.starts_with("- `") {
            for name in backticked(trimmed) {
                out.items.insert(name);
            }
        } else if trimmed.starts_with("| `") {
            // 表格：只取第一格。后面几格是"是什么"，里面的反引号是说明，不是出口。
            let first = trimmed
                .trim_start_matches('|')
                .split('|')
                .next()
                .unwrap_or("");
            for name in backticked(first) {
                out.items.insert(name);
            }
        }
    }
    out
}

fn backticked(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// 源码一侧：从 `lib.rs` 出发，只沿 `pub mod` 往下走。
///
/// 走私有模块是错的：`mod heat;` 里的 `pub struct HeatDiff` 只有在 `mod.rs` 把它
/// `pub use` 出来之后才算出口。反过来，`pub mod` 文件里**直接声明**的 `pub` 项也算——
/// `core/src/paths.rs` 的五个函数就是这么露出来的，没有任何一条 `pub use`。
fn public_surface(member: &str, feature: &str) -> io::Result<Surface> {
    let mut out = Surface::default();
    walk_module(
        &root().join(member).join("src/lib.rs"),
        &root().join(member).join("src"),
        feature,
        &mut out,
    )?;
    Ok(out)
}

fn walk_module(file: &Path, dir: &Path, feature: &str, out: &mut Surface) -> io::Result<()> {
    let text = fs::read_to_string(file)?;
    let lines: Vec<&str> = text.lines().collect();
    let mut cfg = String::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        i += 1;

        if line.starts_with("mod tests") {
            break;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed.starts_with("#[cfg(") {
            cfg = trimmed.to_owned();
            continue;
        }
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        let gated_in = cfg_admits(&cfg, feature);
        cfg = String::new();

        if let Some(rest) = line.strip_prefix("pub mod ") {
            if gated_in {
                let name = rest.trim_end_matches(';').trim().to_owned();
                out.modules.insert(name.clone());
                let as_file = dir.join(format!("{name}.rs"));
                let child = if as_file.is_file() {
                    as_file
                } else {
                    dir.join(&name).join("mod.rs")
                };
                walk_module(&child, &dir.join(&name), feature, out)?;
            }
            continue;
        }

        if line.starts_with("pub use ") {
            let mut spec = line.to_owned();
            while !spec.trim_end().ends_with(';') && i < lines.len() {
                spec.push(' ');
                spec.push_str(lines[i].trim());
                i += 1;
            }
            if gated_in {
                for name in reexported_names(&spec) {
                    out.items.insert(name);
                }
            }
            continue;
        }

        if gated_in && let Some(name) = declared_name(line) {
            out.items.insert(name);
        }
    }
    Ok(())
}

/// 一条 `#[cfg(...)]` 在给定 feature 下放不放行。
///
/// 没有 cfg 就放行；`#[cfg(feature = "x")]` 只在扫 `x` 那一趟放行；其余（`#[cfg(test)]`、
/// `#[cfg(unix)]` 之类）一律不算公共出口——前者不是，后者是平台条件，不该出现在一份
/// 与平台无关的清单里。
fn cfg_admits(cfg: &str, feature: &str) -> bool {
    if cfg.is_empty() {
        return true;
    }
    match cfg.split("feature = \"").nth(1) {
        Some(rest) => rest.split('"').next() == Some(feature),
        None => false,
    }
}

fn reexported_names(spec: &str) -> Vec<String> {
    let body = spec
        .trim()
        .trim_start_matches("pub use ")
        .trim_end_matches(';')
        .trim();

    let inner = match (body.find('{'), body.rfind('}')) {
        (Some(a), Some(b)) if a < b => &body[a + 1..b],
        _ => body,
    };

    inner
        .split(',')
        .map(|part| {
            let one = part.trim();
            let one = one.rsplit(" as ").next().unwrap_or(one);
            one.rsplit("::").next().unwrap_or(one).trim().to_owned()
        })
        .filter(|s| !s.is_empty() && s != "self")
        .collect()
}

/// 一行顶层声明露出来的名字。只看第 0 列：`impl` 块里的 `pub fn` 是缩进的，不是 crate
/// 的出口。`pub(crate)` / `pub(super)` 天然不匹配 `pub ` 这个前缀。
fn declared_name(line: &str) -> Option<String> {
    // 先剥掉可以叠加的修饰词（`pub async fn`、`pub unsafe trait`、`pub const fn`……），
    // 剩下的第一个词才是那个决定"这是什么"的关键字。注意顺序：`const fn` 里的 `const`
    // 是修饰词，而 `pub const MAX: usize` 里的 `const` 是关键字——所以 `fn` 那一条必须
    // 排在 `const` 前面判。
    let mut rest = line.strip_prefix("pub ")?;
    loop {
        let stripped = ["async ", "unsafe ", "extern \"C\" ", "extern "]
            .iter()
            .find_map(|m| rest.strip_prefix(m));
        match stripped {
            Some(r) => rest = r,
            None => break,
        }
    }
    if let Some(r) = rest.strip_prefix("const fn ") {
        return Some(ident(r));
    }
    const KEYWORDS: [&str; 8] = [
        "fn ", "struct ", "enum ", "trait ", "type ", "const ", "static ", "union ",
    ];
    KEYWORDS
        .iter()
        .find_map(|kw| rest.strip_prefix(kw))
        .map(ident)
}

fn ident(text: &str) -> String {
    text.trim()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// 文件里每个类型头上的 `#[derive(...)]`，供 newtype 的 `Deserialize` 扫描使用。键是类型名，值是 derive 所在行号。
fn derived_types(src: &Source) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let mut pending: Option<usize> = None;

    for (no, text) in &src.lines {
        let trimmed = text.trim_start();
        if trimmed.starts_with("#[derive(") {
            pending = trimmed.contains("Deserialize").then_some(*no);
            continue;
        }
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some(line) = pending.take() {
            for kw in ["struct ", "enum "] {
                if let Some(r) = text.strip_prefix("pub ").and_then(|p| p.strip_prefix(kw)) {
                    out.insert(ident(r), line);
                } else if let Some(r) = text.strip_prefix(kw) {
                    out.insert(ident(r), line);
                }
            }
        }
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// 失败消息
// ═══════════════════════════════════════════════════════════════════════════

/// 两侧集合的差，写成"多了什么、少了什么"。
///
/// 只说"不相等"是没用的：这两份清单有几十个名字，人得自己去比。失败消息要能直接照着改。
fn diff_sets(
    problems: &mut Vec<String>,
    scope: &str,
    kind: &str,
    listed: &BTreeSet<String>,
    actual: &BTreeSet<String>,
) {
    for name in actual.difference(listed) {
        problems.push(format!(
            "{scope}: `{name}` is a public {kind} but is not in the README's export list — \
             add it between the `exports:begin` / `exports:end` markers, or stop exporting it"
        ));
    }
    for name in listed.difference(actual) {
        problems.push(format!(
            "{scope}: the README lists `{name}` as a public {kind}, but nothing exports it — \
             remove the line, or check whether it lost its `pub`"
        ));
    }
}

/// 断言一条扫描**确实扫到了东西**。
///
/// 一条匹配不到任何东西的扫描和一条全都通过的扫描，在报告上长得一模一样：绿。而前者是
/// 最糟的一种门禁——它给人"这件事有人管着"的印象，实际什么都没管。上面那条
/// `compile_error!` 的假阳性说明针脚的形态会变；变的方向如果是"一个都匹配不上"，就没有
/// 任何东西会告诉你。
///
/// 用**下界**而不是精确值：精确值会让每加一条日志、每加一个 `select!` 都变红一次，而那
/// 种红没有信息量，只会训练人把数字调大。下界只回答一个问题——针脚还认得出代码吗。
fn assert_saw(kind: &str, found: usize, floor: usize) {
    assert!(
        found >= floor,
        "this scan matched only {found} {kind} (expected at least {floor}) — it is far more \
         likely that the needle stopped matching than that the code lost them. fix the scan \
         before lowering this floor."
    );
}

fn report(problems: &[String]) -> String {
    let mut out = format!("\n{} problem(s):\n", problems.len());
    for p in problems {
        out.push_str("  - ");
        out.push_str(p);
        out.push('\n');
    }
    out
}

/// 遍历一份成员清单的依赖条目，返回 `(行号, 段名, 键, 值)`。
///
/// `[dependencies.foo]` 那种分段写法这里不支持，而且会当场判红：它把一条依赖拆到好几行,
/// 上面两条扫描（裸版本号、testkit 种类）都会漏看。六份清单目前一条都没有，遇到就说清楚,
/// 比悄悄扫不到强。
fn dependency_entries(
    manifest: &str,
    problems: &mut Vec<String>,
    member: &str,
) -> Vec<(usize, String, String, String)> {
    let mut out = Vec::new();
    let mut section = String::new();

    for (idx, line) in manifest.lines().enumerate() {
        let no = idx + 1;
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            let name = trimmed.trim_matches(['[', ']'].as_slice()).to_owned();
            if name.starts_with("dependencies.")
                || name.starts_with("dev-dependencies.")
                || name.starts_with("build-dependencies.")
            {
                problems.push(format!(
                    "{member}/Cargo.toml:{no}: `[{name}]` — write dependencies as one \
                     `key = {{ ... }}` line each; the split form hides them from these scans"
                ));
            }
            section = name;
            continue;
        }
        if !section.ends_with("dependencies") || trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        out.push((
            no,
            section.clone(),
            key.trim().to_owned(),
            value.trim().to_owned(),
        ));
    }
    out
}
