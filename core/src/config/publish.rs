//! 配置的发布与读取。
//!
//! **`watch` 里的那一份就是生效中的配置。** 这是本模块的核心判据，整个模块围着它转。
//!
//! # 读端为什么不直接给 `&Config`
//!
//! `watch::Receiver::borrow()` 返回一个持有读锁的 guard。把它交给调用方，就等于把
//! "别跨 `await` 持有它"变成一条要靠人记住的规矩——而跨 `await` 持锁在关停路径上会
//! 直接变成死锁（`clippy::await_holding_lock` 在本仓库是 `deny`，但它管不到 `watch` 的
//! guard）。
//!
//! 所以 [`ConfigReader::load`] 在函数内部就把 guard 释放掉，交出去的是一份
//! [`ConfigSnapshot`]——`Arc` + 一个整数，克隆代价固定，想拿多久拿多久。
//!
//! # 生成号
//!
//! 每次发布递增。它给半热消费者一个**廉价的"我这份还新吗"判据**：面在启动时固化了一批
//! 值并记下当时的生成号，之后只要比较整数就知道自己是不是落后了，不必逐字段对比配置。
//! 重复应用的抑制（`next <= active` 就跳过）也靠它。

use std::sync::Arc;

use tokio::sync::watch;

use super::types::Config;

/// 配置的版本号。单调递增。
///
/// `Ord`：半热消费者靠 `<=` 抑制重复应用。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// 起始生成号。启动时发布的第一份配置就是它。
    pub const FIRST: Self = Self(0);

    fn next(self) -> Self {
        // 饱和而不是回绕：回绕会让生成号一次性倒退到 0，而所有消费者的
        // `next <= active` 判据会因此**永久**抑制后续重载。
        // u64 在每秒一次重载的节奏下要跑五千亿年才到这里，但代价是一条指令。
        Self(self.0.saturating_add(1))
    }

    /// 底层整数。只用于日志与报告。
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 一份配置及其生成号。
///
/// 克隆代价是一次 `Arc` 计数加一。热字段"每请求现读"读的就是它。
#[derive(Clone, Debug)]
pub struct ConfigSnapshot {
    config: Arc<Config>,
    generation: Generation,
}

impl ConfigSnapshot {
    /// 配置本体。
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 共享所有权的配置本体。要把它塞进一个 `'static` 的任务时用这个。
    #[must_use]
    pub fn shared(&self) -> Arc<Config> {
        Arc::clone(&self.config)
    }

    /// 生成号。
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }
}

/// 一份**已经通过冷段判别**的配置。
///
/// 存在的唯一理由：让"发布一份本该被拒绝的配置"在类型上无从发生。它只由
/// [`evaluate_reload`](super::evaluate_reload) 构造，也只被 [`ConfigPublisher::publish`]
/// 消费，中间没有任何公开的构造途径。
///
/// 没有它的话，装配层就是「`match` 出 `Accept` 分支、然后记得调 `publish`」——
/// 而"记得"正是这一类缺陷的来源。
#[derive(Debug)]
pub struct Accepted(Box<Config>);

impl Accepted {
    pub(super) fn new(config: Config) -> Self {
        Self(Box::new(config))
    }

    /// 看一眼将要发布的内容（报告与测试用）。
    #[must_use]
    pub fn peek(&self) -> &Config {
        &self.0
    }
}

/// 写端。**非 `Clone`**：全进程只有装配层持有它。
///
/// 不可克隆是"没有全局可变状态"的一条直接后果：能写配置的地方有两个，就等于有了一个谁都能改的
/// 全局状态，"真值恒等于生效中的配置"立刻失去意义。
#[derive(Debug)]
pub struct ConfigPublisher {
    tx: watch::Sender<ConfigSnapshot>,
    generation: Generation,
}

impl ConfigPublisher {
    /// 发布初始配置，并给出第一个读端。
    #[must_use]
    pub fn new(initial: Config) -> (Self, ConfigReader) {
        let snapshot = ConfigSnapshot {
            config: Arc::new(initial),
            generation: Generation::FIRST,
        };
        let (tx, rx) = watch::channel(snapshot);
        (
            Self {
                tx,
                generation: Generation::FIRST,
            },
            ConfigReader { rx },
        )
    }

    /// 原子换值，返回新的生成号。
    ///
    /// 用 `send_replace` 而不是 `send`：后者在没有接收端时返回 `Err`，而"没人在听"
    /// 对配置发布来说不是错误——所有面都退出之后重载一次，真值仍然应该被更新。
    /// 把它当错误处理，装配层就得为一个不是问题的情况写一条分支。
    pub fn publish(&mut self, accepted: Accepted) -> Generation {
        self.generation = self.generation.next();
        let snapshot = ConfigSnapshot {
            config: Arc::from(accepted.0),
            generation: self.generation,
        };
        drop(self.tx.send_replace(snapshot));
        self.generation
    }

    /// 当前生效的配置。
    #[must_use]
    pub fn current(&self) -> ConfigSnapshot {
        self.tx.borrow().clone()
    }

    /// 再要一个读端。
    #[must_use]
    pub fn reader(&self) -> ConfigReader {
        ConfigReader {
            rx: self.tx.subscribe(),
        }
    }
}

/// 读端。廉价 `Clone`，每个面各持一个。
#[derive(Clone, Debug)]
pub struct ConfigReader {
    rx: watch::Receiver<ConfigSnapshot>,
}

impl ConfigReader {
    /// 取当前配置。
    ///
    /// 内部的 `watch` guard 在返回前就释放了——调用方拿到的是 `Arc`，跨 `await` 持有它
    /// 不会挡住任何一次发布。
    #[must_use]
    pub fn load(&self) -> ConfigSnapshot {
        self.rx.borrow().clone()
    }

    /// 当前生成号。
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.rx.borrow().generation
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::evaluate_reload;
    use super::super::heat::ReloadDecision;
    use super::*;

    fn accept(current: &Config, next: Config) -> Accepted {
        match evaluate_reload(current, next) {
            ReloadDecision::Accept { config, .. } => config,
            ReloadDecision::RejectCold { diff } => {
                panic!("这一份本该被接受，冷清单是 {:?}", diff.cold_fields())
            }
        }
    }

    #[test]
    fn the_watch_value_is_the_effective_config() {
        // 核心判据：发布之后，读端立刻读到新值。
        let (mut publisher, reader) = ConfigPublisher::new(Config::default());
        assert_eq!(reader.load().config(), &Config::default());

        let mut next = Config::default();
        next.worker.tick_interval = Duration::from_secs(1);
        let generation = publisher.publish(accept(reader.load().config(), next.clone()));

        assert_eq!(reader.load().config(), &next);
        assert_eq!(reader.load().generation(), generation);
        assert_eq!(publisher.current().config(), &next);
    }

    #[test]
    fn generation_advances_by_one_per_publish() {
        let (mut publisher, reader) = ConfigPublisher::new(Config::default());
        assert_eq!(reader.generation(), Generation::FIRST);

        for expected in 1..=3_u64 {
            let mut next = Config::default();
            next.worker.tick_interval = Duration::from_secs(expected);
            let generation = publisher.publish(accept(reader.load().config(), next));
            assert_eq!(generation.get(), expected);
            assert_eq!(reader.generation(), generation);
        }
    }

    #[test]
    fn generations_are_ordered_so_stale_values_can_be_detected() {
        // 半热消费者靠这个比较抑制重复应用。
        let (mut publisher, reader) = ConfigPublisher::new(Config::default());
        let pinned = reader.generation();

        let mut next = Config::default();
        next.worker.enabled = false;
        publisher.publish(accept(reader.load().config(), next));

        assert!(
            pinned < reader.generation(),
            "固化时记下的生成号必须能判出落后"
        );
    }

    #[test]
    fn every_reader_sees_the_same_value() {
        let (mut publisher, first) = ConfigPublisher::new(Config::default());
        let second = publisher.reader();
        let cloned = first.clone();

        let mut next = Config::default();
        next.http.handler_timeout = Duration::from_secs(99);
        publisher.publish(accept(first.load().config(), next.clone()));

        for (i, reader) in [&first, &second, &cloned].iter().enumerate() {
            assert_eq!(reader.load().config(), &next, "TC{i} 的读端没看到新值");
        }
    }

    #[test]
    fn publishing_with_no_readers_left_is_not_an_error() {
        // 所有面都退出之后收到 SIGHUP：真值仍然应该更新，而不是让装配层去处理一个
        // "没人在听"的错误分支。
        let (mut publisher, reader) = ConfigPublisher::new(Config::default());
        let mut next = Config::default();
        next.worker.enabled = false;
        let accepted = accept(reader.load().config(), next.clone());
        drop(reader);

        let generation = publisher.publish(accepted);
        assert_eq!(generation.get(), 1);
        assert_eq!(publisher.current().config(), &next);
    }

    #[test]
    fn a_snapshot_taken_before_a_publish_keeps_its_own_value() {
        // 这是 `load()` 交出 Arc 而不是 guard 的可观察后果：持有旧快照的任务不会
        // 挡住新的发布，也不会在中途看到值被换掉。
        let (mut publisher, reader) = ConfigPublisher::new(Config::default());
        let held = reader.load();

        let mut next = Config::default();
        next.worker.tick_interval = Duration::from_secs(7);
        publisher.publish(accept(reader.load().config(), next));

        assert_eq!(held.config(), &Config::default(), "旧快照必须保持不变");
        assert_eq!(held.generation(), Generation::FIRST);
        assert_ne!(reader.load().generation(), held.generation());
    }
}
