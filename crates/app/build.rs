//! 构建串：编译期注入 git 身份与版本，供启动首条日志使用。
//!
//! 只做一件事：让"现在跑的是哪份代码"可回答。git 不可用（没有仓库、没有提交）时退化为 `unknown`，
//! 不让构建失败——但也不假装知道。

use std::process::Command;

fn main() {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git").args(args).output().ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };

    let sha = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let dirty = match git(&["status", "--porcelain"]) {
        Some(status) if status.is_empty() => "0",
        Some(_) => "1",
        None => "unknown",
    };

    println!("cargo:rustc-env=BUILD_GIT_SHA={sha}");
    println!("cargo:rustc-env=BUILD_GIT_DIRTY={dirty}");
    println!("cargo:rerun-if-changed=build.rs");
}
