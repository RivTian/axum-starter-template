//! Published todo events reach the event log through the bus, also those published just
//! before a shutdown.

use std::error::Error;
use std::time::Duration;

use serde_json::Value;
use svc_domain::todo::{TodoEvent, TodoId, TodoPublisher};
use svc_infra::event_log::EventLog;
use svc_infra::publisher::BusTodoPublisher;
use svc_runtime::bus::EventBus;
use svc_runtime::phase::{Phase, PhaseWatch};
use svc_runtime::settings::LifecycleSettings;
use svc_runtime::signal::Signal;
use svc_runtime::supervisor::Supervisor;
use svc_test_utils::logs::CapturedLogs;
use svc_test_utils::signals::FakeSignals;
use time::macros::datetime;

#[tokio::test]
async fn every_published_event_is_logged_once() -> Result<(), Box<dyn Error>> {
    let (logs, _guard) = CapturedLogs::start();
    let bus = EventBus::<TodoEvent>::new(8);
    let publisher = BusTodoPublisher::new(bus.clone());
    let phases = PhaseWatch::new();
    let mut phase = phases.subscribe();
    let supervisor =
        Supervisor::new(&LifecycleSettings::default(), phases).with(EventLog::new(bus.subscribe()));
    let (signals, sender) = FakeSignals::new();
    let run = tokio::spawn(supervisor.run(signals));
    phase.wait_for(|phase| *phase == Phase::Running).await?;
    let id = TodoId::at(datetime!(2026-01-01 00:00 UTC));
    publisher.publish(TodoEvent::Created { id });
    publisher.publish(TodoEvent::Deleted { id });
    for _ in 0..100 {
        if logs.with_message("todo event").len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    sender.send(Signal::Interrupt);
    assert_eq!(run.await?.name(), "stopped");
    let events = logs.with_message("todo event");
    let names: Vec<&Value> = events.iter().filter_map(|e| e.get("event.name")).collect();
    assert_eq!(names, ["todo_created", "todo_deleted"]);
    assert_eq!(events[0].get("todo.id"), Some(&Value::from(id.to_string())));
    Ok(())
}

#[tokio::test]
async fn events_published_before_a_shutdown_are_still_logged() -> Result<(), Box<dyn Error>> {
    let (logs, _guard) = CapturedLogs::start();
    let bus = EventBus::<TodoEvent>::new(1024);
    let publisher = BusTodoPublisher::new(bus.clone());
    let phases = PhaseWatch::new();
    let mut phase = phases.subscribe();
    let supervisor =
        Supervisor::new(&LifecycleSettings::default(), phases).with(EventLog::new(bus.subscribe()));
    let (signals, sender) = FakeSignals::new();
    let run = tokio::spawn(supervisor.run(signals));
    phase.wait_for(|phase| *phase == Phase::Running).await?;
    let id = TodoId::at(datetime!(2026-01-01 00:00 UTC));
    // More than the event log logs in one go before it yields to the scheduler, so it is
    // still logging them when it is asked to stop.
    for _ in 0..1000 {
        publisher.publish(TodoEvent::Created { id });
    }
    sender.send(Signal::Interrupt);
    assert_eq!(run.await?.name(), "stopped");
    assert_eq!(logs.with_message("todo event").len(), 1000);
    Ok(())
}
