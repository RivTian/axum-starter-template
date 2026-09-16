//! 文件系统夹具：一个假的安装根、一条临时库路径。
//!
//! 两个夹具都靠 `tempfile` 的 drop 清理。门禁会反复跑，留垃圾会拖垮下一轮——所以这里
//! **不提供** "保留目录以便排查" 的开关：想留就在断言消息里把路径打出来，那比一个谁都
//! 记不住要关的全局开关安全。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use service_core::paths;
use tempfile::TempDir;

/// 假可执行文件的缺省名。
const DEFAULT_EXE_NAME: &str = "service";

/// 临时库的缺省文件名。
const DEFAULT_DB_NAME: &str = "service.sqlite3";

/// 一个假的安装根：`<tmp>/` 下放一个假可执行文件，外加 `config/` 与 `data/`。
///
/// 目录布局**由 `core::paths` 自己算出来**，不是在这里抄一遍。这一条是夹具存在的主要理由：
/// 抄一遍的版本在"生产改了布局"那天仍然全绿，而全绿的测试是最贵的一种测试。
///
/// 安装根同样不是直接取 `TempDir::path()`，而是把假可执行文件的路径喂给
/// [`paths::install_root_from`]——于是"锚点 = 可执行文件所在目录"这条规则在夹具里也只有
/// 一个实现。
#[derive(Debug)]
pub struct TempInstallRoot {
    /// 持有它就是持有生命期：drop 时目录树被删掉。
    _dir: TempDir,
    root: PathBuf,
    exe: PathBuf,
}

impl TempInstallRoot {
    /// 造一个安装根，假可执行文件叫 `service`。
    ///
    /// # Errors
    ///
    /// 建临时目录或建子目录失败时返回 [`io::Error`]。
    pub fn new() -> io::Result<Self> {
        Self::with_exe_name(DEFAULT_EXE_NAME)
    }

    /// 同 [`Self::new`]，但指定假可执行文件的名字。
    ///
    /// 名字会影响什么：`app` 的一部分诊断信息里带可执行文件名。要断言那些信息就需要控制它。
    ///
    /// # Errors
    ///
    /// 建临时目录、建子目录或写假可执行文件失败时返回 [`io::Error`]。
    pub fn with_exe_name(exe_name: &str) -> io::Result<Self> {
        let dir = TempDir::new()?;
        let exe = dir.path().join(exe_name);

        // 这是**生产那条规则**，不是夹具自己的一条平行规则。
        let root = paths::install_root_from(&exe);

        fs::create_dir_all(paths::config_dir(&root))?;
        fs::create_dir_all(paths::data_dir(&root))?;

        // 内容为空、也不设可执行位：没有任何一层去 exec 它。`current_exe()` 的返回值在本
        // 模板里只被当作**一个路径**用（它到底返回什么，本模板举不出自动化证据），
        // 夹具照着这个用法造就够了。设可执行位要走 `std::os::unix`，那会把这个 crate 绑到
        // 一个我们并未验证过的平台集合上。
        fs::write(&exe, b"")?;

        Ok(Self {
            _dir: dir,
            root,
            exe,
        })
    }

    /// 安装根。
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 假可执行文件的路径。喂给 `ProcessEnv` 的就是它。
    #[must_use]
    pub fn exe_path(&self) -> &Path {
        &self.exe
    }

    /// `<root>/config`。
    #[must_use]
    pub fn config_dir(&self) -> PathBuf {
        paths::config_dir(&self.root)
    }

    /// `<root>/data`。
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        paths::data_dir(&self.root)
    }

    /// `<root>/config/service.toml`。
    #[must_use]
    pub fn default_config_path(&self) -> PathBuf {
        paths::default_config_path(&self.root)
    }

    /// 把 `body` 写进缺省配置路径，返回那条路径。
    ///
    /// # Errors
    ///
    /// 写文件失败时返回 [`io::Error`]。
    pub fn write_config(&self, body: &str) -> io::Result<PathBuf> {
        let path = self.default_config_path();
        fs::write(&path, body)?;
        Ok(path)
    }

    /// 把 `body` 写进 `<root>/config/<name>`，返回那条路径。
    ///
    /// 用于 `--config` 那一档：三段优先级里显式指定文件的那一段需要一条**不是**缺省名的路径。
    ///
    /// # Errors
    ///
    /// 写文件失败时返回 [`io::Error`]。
    pub fn write_config_as(&self, name: &str, body: &str) -> io::Result<PathBuf> {
        let path = self.config_dir().join(name);
        fs::write(&path, body)?;
        Ok(path)
    }
}

/// 一条临时的 SQLite 文件路径。
///
/// **文件本身刻意不创建。** 「不存在时把库建出来」是 `storage::open` 的职责之一，夹具先把
/// 文件摆好就等于把那条路径测没了——而那恰恰是首次部署会走的路径。
#[derive(Debug)]
pub struct TempDb {
    _dir: TempDir,
    path: PathBuf,
}

impl TempDb {
    /// 造一条临时库路径。
    ///
    /// # Errors
    ///
    /// 建临时目录失败时返回 [`io::Error`]。
    pub fn new() -> io::Result<Self> {
        let dir = TempDir::new()?;
        let path = dir.path().join(DEFAULT_DB_NAME);
        Ok(Self { _dir: dir, path })
    }

    /// 库文件路径。调用时它**还不存在**。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 库文件现在存在吗。
    ///
    /// 「`open` 之后文件被建出来了」是一条值得断言的事实，而它需要一个前后对比。
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_anchor_rule_is_the_production_one() {
        // 夹具不自己定义"安装根是什么"。这条用例守的就是这一点：若哪天有人把 `root` 改成
        // 直接取 `TempDir::path()`，锚点规则就出现了第二个实现，而这两个实现迟早会分叉。
        let fixture = TempInstallRoot::new().expect("建安装根");
        assert_eq!(
            fixture.root(),
            paths::install_root_from(fixture.exe_path()),
            "安装根必须等于把假可执行文件喂给生产函数得到的结果"
        );
        assert_eq!(
            fixture.exe_path().file_name(),
            Some(DEFAULT_EXE_NAME.as_ref())
        );
    }

    #[test]
    fn layout_is_created_and_matches_core_paths() {
        let fixture = TempInstallRoot::new().expect("建安装根");
        let cases: &[(&str, PathBuf)] = &[
            ("config", fixture.config_dir()),
            ("data", fixture.data_dir()),
        ];
        for (i, (name, dir)) in cases.iter().enumerate() {
            assert!(
                dir.is_dir(),
                "TC{i} ({name}) 目录没建出来：{}",
                dir.display()
            );
            assert_eq!(
                dir.parent(),
                Some(fixture.root()),
                "TC{i} ({name}) 不在根下"
            );
        }
        assert!(fixture.exe_path().is_file(), "假可执行文件没写出来");
        assert!(
            !fixture.default_config_path().exists(),
            "缺省配置文件不该被预先创建——「文件不存在」是三段优先级里要覆盖的一档"
        );
    }

    #[test]
    fn a_different_fixture_is_a_different_anchor() {
        // 编排层要用三份不同的 exe_path 断言配置解析随锚点走。前提是夹具之间确实不同。
        let a = TempInstallRoot::new().expect("建 a");
        let b = TempInstallRoot::new().expect("建 b");
        assert_ne!(a.root(), b.root());
        assert_ne!(a.default_config_path(), b.default_config_path());
        assert_ne!(a.data_dir(), b.data_dir());
    }

    #[test]
    fn config_can_be_written_at_the_default_name_and_at_another_one() {
        let fixture = TempInstallRoot::new().expect("建安装根");

        let default_path = fixture.write_config("# default\n").expect("写缺省配置");
        assert_eq!(default_path, fixture.default_config_path());
        assert_eq!(
            fs::read_to_string(&default_path).expect("读回"),
            "# default\n"
        );

        let other = fixture
            .write_config_as("override.toml", "# override\n")
            .expect("写具名配置");
        assert_ne!(other, default_path, "`--config` 那一档需要一条不同的路径");
        assert_eq!(other.parent(), Some(fixture.config_dir().as_path()));
    }

    #[test]
    fn temp_db_hands_out_a_path_that_does_not_exist_yet() {
        let db = TempDb::new().expect("建临时库");
        assert!(
            !db.exists(),
            "库文件必须还不存在——建库是 `storage::open` 的职责，夹具先摆好就把那条路径测没了"
        );
        assert_eq!(db.path().file_name(), Some(DEFAULT_DB_NAME.as_ref()));

        // 建出来之后 `exists()` 要能看见变化，否则这个方法做不了前后对比。
        fs::write(db.path(), b"").expect("摸一下文件");
        assert!(db.exists());
    }

    #[test]
    fn everything_is_removed_on_drop() {
        let (root, db) = {
            let fixture = TempInstallRoot::new().expect("建安装根");
            let db = TempDb::new().expect("建临时库");
            fs::write(db.path(), b"x").expect("写库文件");
            (fixture.root().to_path_buf(), db.path().to_path_buf())
        };
        assert!(
            !root.exists(),
            "安装根应随 drop 一起消失：{}",
            root.display()
        );
        assert!(!db.exists(), "临时库应随 drop 一起消失：{}", db.display());
    }
}
