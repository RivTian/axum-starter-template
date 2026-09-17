        let notes = svc_storage::NotesRepo::new(&db);
        supervisor.register(TaskSpec::new(
            TaskKey::new("notes-writer"),
            RuntimeId::MAIN,
            move |ctx: TaskContext| {
                let notes = notes.clone();
                Box::pin(async move { crate::writer::run(ctx, notes).await })
            },
        ))?;
