//! Desktop notification posted when a top-level chat goes idle (the
//! `notifyOnIdle` setting). macOS only; other platforms do nothing.

use crate::worker::{QueuePriority, QueuedItem};

/// Whether a delivered batch carries work a person sent, so the park after it
/// is worth a notification. Unattended deliveries (heartbeat and cron fires,
/// agent messages, environment wakes, injected continuations) never arm it:
/// otherwise every scheduled fire and every scripted turn would post one.
pub(crate) fn batch_awaits_person(items: &[QueuedItem]) -> bool {
    items.iter().any(|item| match item.priority {
        QueuePriority::Human | QueuePriority::Pinned => true,
        QueuePriority::Background => false,
    })
}

/// The notification body, shared by the daemon post and the attached
/// client's `idle_notification` session event.
pub(crate) const IDLE_NOTIFICATION_BODY: &str = "Waiting for your review or reply";

/// The notification title: the chat's name, or the working folder while the
/// chat has no name yet (the automatic title lands after the first settled
/// turn).
pub(crate) fn idle_notification_title(chat_name: Option<&str>, cwd: &str) -> String {
    chat_name.map_or_else(
        || {
            let folder = std::path::Path::new(cwd).file_name().map_or_else(
                || cwd.to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            format!("Prime Env chat in {folder}")
        },
        str::to_string,
    )
}

/// Posts the "waiting for you" notification through `osascript`, for a
/// session no client is attached to (an attached client posts it through
/// its own terminal instead, so the notice comes from that app). Fire and
/// forget: a failed spawn is logged, never surfaced to the session.
pub(crate) fn post_idle_notification(title: &str) {
    if !cfg!(target_os = "macos") {
        return;
    }
    // The text travels as argv items, so it needs no AppleScript escaping.
    let spawned = tokio::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 1 of argv) with title (item 2 of argv)",
            "-e",
            "end run",
            "--",
        ])
        .arg(IDLE_NOTIFICATION_BODY)
        .arg(title)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false)
        .spawn();
    match spawned {
        Ok(mut child) => {
            tokio::spawn(async move {
                if let Err(error) = child.wait().await {
                    eprintln!("pa-daemon idle notification did not finish: {error}");
                }
            });
        }
        Err(error) => eprintln!("pa-daemon idle notification could not start: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::TurnPolicy;

    fn item(priority: QueuePriority, queue_key: Option<&str>) -> QueuedItem {
        QueuedItem {
            message: "work".to_string(),
            priority,
            preview: None,
            custom_message: None,
            agent_message: None,
            queue_key: queue_key.map(str::to_string),
            admission_id: None,
            images: Vec::new(),
            done: None,
            queue_visible: true,
            policy: TurnPolicy::Injected,
            forced_batch: false,
        }
    }

    #[test]
    fn only_batches_with_person_sent_work_arm_the_notification() {
        let heartbeat = item(QueuePriority::Background, Some("heartbeat:job-1"));
        let wake = item(QueuePriority::Background, Some("environment_wake"));
        let prompt = item(QueuePriority::Human, None);
        let pinned = item(QueuePriority::Pinned, None);
        let verdicts = [
            batch_awaits_person(std::slice::from_ref(&heartbeat)),
            batch_awaits_person(std::slice::from_ref(&wake)),
            batch_awaits_person(&[heartbeat, prompt]),
            batch_awaits_person(&[pinned]),
        ];
        assert_eq!(verdicts, [false, false, true, true]);
    }
}
