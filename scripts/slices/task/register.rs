        supervisor.register(TaskSpec::new(
            TaskKey::new("flush"),
            RuntimeId::MAIN,
            |ctx: TaskContext| Box::pin(async move { crate::flush::run(ctx).await }),
        ))?;
